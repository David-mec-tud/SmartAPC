import { test, expect } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { seed } from "./fixtures";

test("I/O CSV example imports all groups atomically and custom variables survive manual editing and reload", async ({
  page,
  request,
}) => {
  const name = `journey_io_${Date.now()}`;
  expect(
    (await request.post("/api/projects", { data: { name } })).ok(),
  ).toBeTruthy();
  await page.goto("/configuration/io");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  const pending = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download I/O CSV example" }).click();
  const example = await readFile((await (await pending).path())!, "utf8");
  expect(example).toContain("group,name,signal_type,read_node,write_node");
  expect(example).toContain("custom,interlock,bool");
  const upload = (csv: string) =>
    page
      .getByLabel("Import I/O CSV", { exact: true })
      .setInputFiles({
        name: "io.csv",
        mimeType: "text/csv",
        buffer: Buffer.from(csv),
      });
  await upload(example.replace("ns=2;s=Indicator", "invalid-node"));
  await expect(page.getByRole("alert")).toContainText(
    "not a valid OPC UA Node ID",
  );
  await expect(
    page.getByRole("button", { name: "MVS 0", exact: true }),
  ).toBeVisible();
  await upload(example);
  await expect(
    page.getByText(
      "I/O definitions imported into the draft. Save configuration to keep them.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "MVS 1", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "CVS 1", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "DVS 1", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Custom variables 3", exact: true })
    .click();
  await expect(page.getByLabel("interlock Type", { exact: true })).toHaveValue(
    "bool",
  );
  await expect(
    page.getByLabel("indicator Read Node ID", { exact: true }),
  ).toHaveValue("");
  await expect(
    page.getByLabel("auxiliary Write Node ID", { exact: true }),
  ).toHaveValue("ns=2;s=Auxiliary");
  expect(
    (await (await request.get(`/api/projects/${name}/config`)).json())
      .io_definition.custom,
  ).toHaveLength(0);
  await page.getByLabel("New variable name").fill("permission");
  await page.getByRole("button", { name: "Add variable", exact: true }).click();
  await page
    .getByLabel("permission Type", { exact: true })
    .selectOption("bool");
  await page
    .getByLabel("permission Read Node ID", { exact: true })
    .fill("ns=2;s=Permission");
  await page
    .getByRole("button", { name: "Save configuration", exact: true })
    .click();
  await expect(
    page.getByText("Configuration saved and normalized by Rust."),
  ).toBeVisible();
  await page.reload();
  await page
    .getByRole("button", { name: "Custom variables 4", exact: true })
    .click();
  await expect(
    page.getByLabel("permission Read Node ID", { exact: true }),
  ).toHaveValue("ns=2;s=Permission");
  const saved = await (
    await request.get(`/api/projects/${name}/config`)
  ).json();
  expect(
    saved.io.inputs.find(
      (point: { name: string }) => point.name === "custom.permission.read",
    ).signal_type,
  ).toBe("bool");
  expect(
    saved.io.outputs.some(
      (point: { name: string }) => point.name === "custom.permission.write",
    ),
  ).toBeFalsy();
});

test("a delayed I/O CSV import preserves newer edits and cannot cross projects", async ({
  page,
  request,
}) => {
  const name = await seed(request, false);
  const other = await seed(request, false);
  await page.goto("/configuration/io");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
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
  await page.route("**/io/import", async (route) => {
    arrived();
    await gate;
    await route.continue();
  });
  const upload = () =>
    page
      .getByLabel("Import I/O CSV", { exact: true })
      .setInputFiles({
        name: "io.csv",
        mimeType: "text/csv",
        buffer: Buffer.from("group,name,read_node\ncustom,late,ns=2;s=Late\n"),
      });
  await upload();
  await started;
  await page
    .getByLabel("u1 Read Node ID", { exact: true })
    .fill("ns=2;s=NewRead");
  release();
  await expect(page.getByRole("alert")).toContainText(
    "Configuration changed during the CSV request",
  );
  await expect(page.getByLabel("u1 Read Node ID", { exact: true })).toHaveValue(
    "ns=2;s=NewRead",
  );
  gate = hold();
  started = new Promise<void>((resolve) => {
    arrived = resolve;
  });
  await upload();
  await started;
  await page.getByLabel("Editing project", { exact: true }).selectOption(other);
  const response = page.waitForResponse((response) =>
    response.url().endsWith(`/projects/${name}/io/import`),
  );
  release();
  await response;
  await expect(
    page.getByRole("button", { name: "Custom variables 0", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("u1 Read Node ID", { exact: true })).toHaveValue(
    "ns=2;s=U1_READ",
  );
});

test("custom numeric and Boolean I/O tests use only the configured directions on the virtual plant", async ({
  page,
  request,
}) => {
  const name = `journey_custom_hardware_${Date.now()}`;
  await request.post("/api/projects", { data: { name } });
  const response = await request.get(`/api/projects/${name}/config`);
  const cfg = await response.json();
  cfg.communication.endpoint_url = "opc.tcp://127.0.0.1:14856/";
  cfg.io_definition.custom = [
    {
      name: "aux",
      signal_type: "f64",
      read: "ns=2;s=u1_read",
      write: "ns=2;s=u1_write",
    },
    {
      name: "permit",
      signal_type: "bool",
      read: "ns=2;s=ExternalControllerEnable",
    },
    { name: "lamp", signal_type: "bool", write: "ns=2;s=ControllerHasControl" },
  ];
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: cfg,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/configuration/io");
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await page
    .getByRole("button", { name: "Run read test", exact: true })
    .click();
  await expect(
    page
      .getByRole("region", { name: "Read test result" })
      .getByText("Passed", { exact: true }),
  ).toBeVisible({ timeout: 20_000 });
  await expect(
    page
      .getByRole("region", { name: "Read test result" })
      .getByRole("row", { name: /custom.permit.read/ }),
  ).toContainText("false");
  await expect(
    page.getByLabel("custom.permit.write test value", { exact: true }),
  ).toHaveCount(0);
  await page
    .getByLabel("custom.aux.write", { exact: true })
    .first()
    .fill("1.75");
  await page
    .getByLabel("custom.lamp.write test value", { exact: true })
    .selectOption("true");
  await page
    .getByRole("button", { name: "Write entered test values", exact: true })
    .click();
  await expect(
    page
      .getByRole("region", { name: "Write test result" })
      .getByText("Passed", { exact: true }),
  ).toBeVisible({ timeout: 20_000 });
  await expect
    .poll(
      async () =>
        (
          await (
            await request.get("http://127.0.0.1:18082/api/virtual-hardware")
          ).json()
        ).latest.sample.u,
    )
    .toBe(1.75);
});
