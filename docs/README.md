# SmartAPC GitHub Pages showcase

`index.html` is the entry point for the public feature showcase. Styles, the
optional screenshot-gallery script, and demo images live in `assets/`. This is a
static site with no build step, external fonts, CDN, API calls, or Rust backend.
All asset paths are relative, including when hosted at `/SmartAPC/`.

## Publish on GitHub Pages

1. Push the commit containing these files to `main`.
2. In the repository, open **Settings → Pages → Build and deployment**.
3. Set **Source** to **Deploy from a branch**.
4. Select **main**, choose **/docs**, and save.
5. Use **Visit site** once GitHub reports a successful deployment.

The expected project URL is <https://david-mec-tud.github.io/SmartAPC/>. Adding
these files does not enable Pages or publish the repository automatically.
`.nojekyll` lets GitHub serve the HTML and assets without Jekyll processing.
Existing Markdown documentation stays in this folder and is linked on GitHub.

See GitHub's [publishing-source instructions](https://docs.github.com/de/pages/getting-started-with-github-pages/configuring-a-publishing-source-for-your-github-pages-site)
and [entry-point requirements](https://docs.github.com/de/pages/getting-started-with-github-pages/creating-a-github-pages-site).

## Preview locally

From the repository root:

```sh
python3 -m http.server 8090 --directory docs --bind 127.0.0.1
```

Open <http://127.0.0.1:8090/>. Check desktop and mobile layouts, the gallery's
arrow/Home/End keyboard navigation, and full-size image links. With JavaScript
disabled, all gallery figures remain visible. The site also works by opening
`index.html` directly.

## Demo image provenance

The four `assets/demo-*.png` images are actual browser captures at 1440 × 1000,
using a disposable copy of `examples/demo_project.toml` and the bundled frontend.
They contain demonstration data, not production measurements. Capture against a
separate temporary `--project-root` so existing projects and hardware are untouched.

- `demo-trends.png`: **Operation → Trends**, with the demo's simulated CV/MV
  history, target bands, and MPC forecasts.
- `demo-strategy.png`: **Configuration → Control strategy**, Linear MPC settings.
- `demo-simulation.png`: **Engineering → Simulation**, saved simulation settings.
- `demo-deployment.png`: **Deployment → Validate & run**, stopped demo project.

For the trend capture, use the demo model, target 10, band offsets −0.5/+0.5,
and a 1-second controller cycle. A 2-column, 1-row dashboard and a 120-second
history window show both plots together. Run simulation at 10× speed, then stop
after the response has developed. No OPC UA connection is needed for simulation.
Retain screenshots' intrinsic dimensions in `index.html` when replacing them.
