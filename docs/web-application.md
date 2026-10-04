# SmartAPC browser workspace

The migration is in progress. The React workspace is served by Axum and calls
Rust services; native UI helpers are retained until the full coverage audit passes.
See [migration coverage](web-migration-coverage.md) for outstanding verification.

## Development

Prerequisites: the existing Rust/native build toolchain and Node.js 22 or newer.
Node is used only to build/test the frontend. Existing projects remain TOML with
SQLite history and structured JSONL logs.

```sh
cd frontend
npm ci
npm run dev
```

In a second terminal from the repository root:

```sh
cargo run -- serve --project-root ./projects
```

Open `http://127.0.0.1:5173`. Vite proxies `/api` and the local Plotly asset to
Axum on port 8080. Production and development use the same API contracts.

## Production assets and launch

```sh
cd frontend
npm ci
npm run build
cd ..
cargo run -- serve --project-root ./projects
```

Open `http://127.0.0.1:8080`. The production UI is served from `frontend/dist`
in a source checkout. It uses local scripts, system fonts, bundled icons and
`assets/plotly-2.35.2.min.js`, with no CDN requests. Direct SPA navigation and
refresh use `index.html`; missing API routes return JSON 404 errors, and missing
static files remain 404 responses.

For a copied executable, place the entire `frontend/dist` directory under `ui/`
next to the executable. Axum looks there first, independently of the launch
directory. Alternatively set `SMARTAPC_UI_DIR` to an absolute asset directory.
Use an absolute `--project-root` when launching from arbitrary directories.
Production packaging and headless helper parity still need the final coverage
checks. Native dependencies have not yet been removed from the default build.

## Validation

```sh
cargo fmt
cargo check
cargo test
cd frontend
npm run typecheck
npm run build
npm test
npx playwright install chromium
npm run test:e2e
```

Playwright starts a real Rust backend on port 18080 and creates isolated projects
under `/private/tmp/smartapc-playwright-projects`. It uses mock communication and
simulation; it does not write to existing controller hardware. Set
`SMARTAPC_TEST_URL` only to a deliberately isolated test backend when reusing one.
Local OPC UA integration tests and browser tests require localhost listener access.

## API and ownership

Typed wire DTOs live in `frontend/src/api.ts`. Rust integration tests exercise
the Axum routes with actual project/runtime/simulation services. Keep DTOs and
contract tests aligned when adding fields. Default serde field names remain
unchanged for existing project compatibility.

| Route | Behavior |
| --- | --- |
| `GET /api/projects/{name}/config` | Saved config and `ETag` revision |
| `PUT /api/projects/{name}/config` | Requires matching `If-Match`; returns normalized saved config and new `ETag` |
| `GET /api/projects/{name}/config/download` | Browser TOML download |
| `POST /api/projects/{name}/config/parse` | Parse uploaded TOML into a draft; does not save |
| `GET /api/projects/{name}/engineering` | Active parameters, sweep dimensions, observer schema, safety limits and engineering availability |
| `GET /api/projects/{name}/readiness` | Saved configuration readiness and supported local command; no hardware writes |
| `POST /api/projects/{name}/jobs` | `strategy_build`, `control_tests`, `kalman_replay`, `offline_optimization`, `io_read`, `io_write`, `watchdog_test` |
| `GET /api/projects/{name}/jobs` | Recent jobs, including saved terminal reports |
| `GET /api/projects/{name}/jobs/{id}` | Project-scoped job progress/result/error |
| `POST /api/projects/{name}/jobs/{id}/cancel` | Request cancellation where supported (offline optimization) |
| `POST /api/projects/{name}/jobs/{id}/apply` | Apply an offline result only while stopped and with unchanged project revision |
| `POST /api/projects/{name}/strategy/template` | Generate project-local Rust source; refuses silent overwrites |
| `GET/PUT /api/projects/{name}/strategy/source` | Read/save project-local source; save invalidates build compatibility |
| `PUT /api/projects/{name}/online-optimization/config` | Save objective/search configuration with a revision; locks during optimization |
| `GET /api/projects/{name}/history?mode=runtime\|simulation&window_ms=180000` | Project-scoped SQLite history, including stopped execution |
| `GET /api/projects/{name}/history/catalog?mode=runtime\|simulation` | Recorded series catalog |
| `GET /api/projects/{name}/logs?mode=runtime\|simulation` | At most 200 parsed JSONL entries from a bounded file tail |
| `POST /api/execution/command` | Explicit `{project_name, mode, action: "start"\|"stop"}` |
| `POST /api/execution/parameters` | `{project_name, mode, params}`; typed active runtime or simulation mutation and persistence |
| `POST /api/execution/simulation-config` | Apply normalized simulation settings explicitly to the matching active simulation |
| `GET/POST /api/execution/optimization` | Status or project/mode-scoped start/stop/save-best commands |
| `GET /api/workspace` | Execution, optimization and server timestamp; polling fallback |
| `GET /api/events` | SSE snapshots at one second; disconnect does not stop backend execution |

HTTP mutations are serialized across browser clients. Structural configuration,
source editing and engineering jobs lock a project while it executes; background
jobs also lock the project against conflicting saves/start actions. Manual online
parameter writes retain the controller's optimization lock. Browser drafts remain
separate from authoritative saved/applied values, including after errors or project
selection changes. Legacy API routes and Plotly pages remain available during the
transition; `/legacy` provides the previous basic web page.

Terminal job reports are stored in `projects/<name>/data/jobs/<id>.json` with
project/revision identity. Active jobs are owned by the backend process, survive
browser disconnects, and are not resumed automatically after backend shutdown.

## Deployment support

Deployment readiness distinguishes required strategy/I/O/model configuration from
optional simulation and tuning. It does not imply that hardware has been contacted.
Starting runtime is an explicit action that connects to OPC UA and enables output
writes. Native fault handling and timing ownership remain in Rust.

Windows commands can be copied/downloaded. This is command generation only:
SmartAPC does not currently implement or verify a Windows Service Control Manager
lifecycle. The UI does not install/remove services or publish to an external host.
