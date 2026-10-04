import { test, expect } from "@playwright/test";

test("packaged executable serves refreshed deep links and all assets with external network blocked", async ({
  page,
  request,
}) => {
  const origin = process.env.SMARTAPC_PACKAGED_URL;
  test.skip(
    !origin,
    "Build and launch the portable package, then set SMARTAPC_PACKAGED_URL.",
  );
  const name = `package_${Date.now()}`;
  expect(
    (await request.post(`${origin}/api/projects`, { data: { name } })).ok(),
  ).toBeTruthy();
  const assets: { url: string; status: number }[] = [];
  page.on("response", (response) => {
    if (/\.(js|css)(\?|$)/.test(response.url()))
      assets.push({ url: response.url(), status: response.status() });
  });
  await page.route("**/*", (route) =>
    new URL(route.request().url()).origin === origin
      ? route.continue()
      : route.abort(),
  );
  await page.goto(`${origin}/configuration/io`);
  await page.getByLabel("Editing project", { exact: true }).selectOption(name);
  await expect(
    page.getByRole("heading", { name: "Variables & I/O", exact: true }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Variables & I/O", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Editing project", { exact: true })).toHaveValue(
    name,
  );
  expect(assets.length).toBeGreaterThan(1);
  expect(
    assets.every(
      (asset) => asset.status === 200 && asset.url.startsWith(origin!),
    ),
  ).toBe(true);
  const missing = await request.get(`${origin}/api/unknown-route`);
  expect(missing.status()).toBe(404);
  expect(missing.headers()["content-type"]).toContain("application/json");
  expect((await request.get(`${origin}/assets/missing.js`)).status()).toBe(404);
  const plotly = await request.get(`${origin}/assets/plotly-2.35.2.min.js`);
  expect(plotly.ok()).toBeTruthy();
  expect(plotly.headers()["content-type"]).toContain("javascript");
});
