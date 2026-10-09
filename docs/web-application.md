# SmartAPC browser workspace

The React workspace is served by Axum and calls Rust services. The default build
is independent of native UI libraries. Transitional native helpers remain under
`legacy-native`. See [migration coverage](web-migration-coverage.md) for evidence
and platform limitations.

## Development

Build prerequisites: Rust (including a C/C++ toolchain and CMake for OPC UA/OSQP),
Node.js 22 or newer, and Python 3 for the optional packaging script.
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
Build the host-native relocatable release package:

```sh
python3 scripts/package-web.py --output ./dist/smartapc-web
./dist/smartapc-web/smartapc serve --project-root /absolute/path/to/projects
```

The destination must be new or empty. Packaging runs `npm ci`, the production
frontend build and `cargo build --locked --release`, then writes the executable,
adjacent `ui/`, launch notes and SHA-256 manifest. Use `--profile debug` for a
faster development package. Build/cache dependencies must be available; deployed
operation needs neither Python nor Node. Custom strategy builds require Cargo.
The script packages the current host; Windows/Linux binary execution requires
building and validation on those hosts.

`cargo run --features legacy-native -- desktop` retains the native UI.
`cargo run -- virtual-hardware` serves the OPC UA plant on port 4855 plus a browser
feedback page at `http://127.0.0.1:8081/virtual-hardware`. Override HTTP with
`--web-port`, use `--no-web` for OPC UA only, or use
`cargo run --features legacy-native -- virtual-hardware --native-plot` for native
feedback. The browser page charts a bounded live history and applies manual
MV/target/limit values on plant ticks, independently of open tabs.

## Validation

Hardware tests show their results beside the controls in **Connection & timing**
and **Variables & I/O**. A red **Test failed** indicator includes a short reason;
editing or saving configuration and starting a retry keep the failure visible
until that test succeeds. Read and write results remain independent. Read reports
show values and errors per Node ID, including partial failures, and successful
write tests confirm the number of values sent. Completed reports return after
navigation or refresh and stay scoped to their project. Watchdog tests check a
single counter read and mirror write; they do not measure counter freshness over
the configured timeout. Save configuration drafts before running hardware tests.

```sh
cargo fmt
cargo check
cargo test
cargo check --features legacy-native
cargo test --features legacy-native
cd frontend
npm run typecheck
npm run build
npm test
npx playwright install chromium
npm run test:e2e
```

Playwright starts a real Rust backend on port 18080 and a disposable OPC UA plant
on port 14856 (browser feedback 18082). Projects are isolated under
`/private/tmp/smartapc-playwright-projects`. Real I/O writes target this plant only;
other journeys use mock communication or offline simulation. Set
`SMARTAPC_TEST_URL` only to a deliberately isolated test backend when reusing one.
Local OPC UA integration tests and browser tests require localhost listener access.
For a launched portable package, validate assets with:

```sh
SMARTAPC_TEST_URL=http://127.0.0.1:18083 \
SMARTAPC_PACKAGED_URL=http://127.0.0.1:18083 npm run test:e2e -- --grep packaged
```

Launch that package separately on port 18083 with an isolated project root. This
journey blocks external browser requests and checks refreshed deep links and all
local JS/CSS/Plotly assets.

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
| `GET /api/projects/{name}/jobs` | At most 64 recent job status summaries; full results omitted |
| `GET /api/projects/{name}/jobs/{id}` | Project-scoped job progress/result/error |
| `POST /api/projects/{name}/jobs/{id}/cancel` | Request cancellation where supported (offline optimization) |
| `POST /api/projects/{name}/jobs/{id}/apply` | Apply an offline result only while stopped and with unchanged project revision |
| `POST /api/projects/{name}/strategy/template` | Generate/regenerate source; back up replaced files and saved TOML under `logic/backups/<id>` |
| `GET/PUT /api/projects/{name}/strategy/source` | Read/save project-local source; checks configuration and source revisions; save invalidates compatibility |
| `PUT /api/projects/{name}/online-optimization/config` | Save objective/search configuration with a revision; locks during optimization |
| `GET /api/projects/{name}/history?mode=runtime\|simulation&window_ms=180000` | SQLite window `{samples, original_count, downsampled, sample_limit}`; at most 20,000 samples, preserving extrema |
| `GET /api/projects/{name}/history/catalog?mode=runtime\|simulation` | Recorded series catalog |
| `GET /api/projects/{name}/logs?mode=runtime\|simulation` | At most 200 parsed JSONL entries from a bounded file tail |
| `POST /api/execution/command` | Explicit `{project_name, mode, action: "start"\|"stop", execution_id}`; ID required for stop |
| `GET /api/execution/parameters?project_name=...&mode=...&execution_id=...` | Scoped applied values and execution identity |
| `POST /api/execution/parameters` | `{project_name, mode, execution_id, params, expected_params}`; typed mutation rejects stale applied values |
| `POST /api/execution/simulation-config` | Apply normalized simulation settings explicitly to the matching active simulation, including its execution ID |
| `GET/POST /api/execution/optimization` | Status or project/mode/execution-ID-scoped start/stop/save-best commands |
| `GET /api/workspace` | Execution, optimization and server timestamp; polling fallback |
| `GET /api/events` | SSE snapshots at one second; disconnect does not stop backend execution |

HTTP mutations are serialized across browser clients. Structural configuration,
source editing and engineering jobs lock a project while it executes; background
jobs also lock the project against conflicting saves/start actions. Manual online
parameter writes retain the controller's optimization lock. Browser drafts remain
separate from authoritative saved/applied values, including after errors or project
selection changes. Legacy API routes and Plotly pages remain available; `/legacy`
provides the previous basic web page.

Terminal job reports are stored in `projects/<name>/data/jobs/<id>.json` with
project/revision identity; compact status headers use `<id>.summary.json`. Lists
omit large replay/curve arrays, and expanded reports fetch the full job by ID.
Active jobs are owned by the backend process, survive
browser disconnects, and are not resumed automatically after backend shutdown.

## Deployment support

Deployment readiness distinguishes required strategy/I/O/model configuration from
optional simulation and tuning. It does not imply that hardware has been contacted.
Starting runtime is an explicit action that connects to OPC UA and enables output
writes. Native fault handling and timing ownership remain in Rust.

Windows commands can be copied/downloaded. This is command generation only:
SmartAPC does not currently implement or verify a Windows Service Control Manager
lifecycle. The UI does not install/remove services or publish to an external host.


## Data freshness and controller isolation

SSE sends workspace snapshots at one second with a bounded per-client queue;
HTTP polling resumes when the stream is unavailable. Data age and connection
errors are visible in the shell. Neither transport changes communication, I/O or
logic periods. Commands reject another project, mode, or previous execution ID.
Source/configuration drafts survive route/project selection changes and failed
saves; explicit reload/discard replaces them. Concurrent online edits preserve the
entered draft and offer Reload applied value after a conflict.

Browser history retains first/last/min/max per signal/time bucket, including
boolean transitions, and reports downsampling. Plotly keeps zoom/legend state
across live updates. Forecasts use their generation time and controller step
indices, anchored to the corresponding measured cycle. Simulation logs flush after
64 records or one wall-clock second at the next cycle, so user logs become visible
during real-time execution without redefining control timing.
