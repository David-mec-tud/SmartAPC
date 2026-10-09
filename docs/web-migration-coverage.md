# Browser migration coverage

The migration is implemented against `docs/prompts/smartapc-web-app-goal.md`,
`AGENTS.md`, and the actual native actions in `src/desktop.rs`. The original
uncommitted controller, CV-band, parameter, documentation and test changes are
part of the migrated working baseline and remain outside migration commits.

## Implementation sequence

1. Extract shared strategy/engineering services and typed, project-scoped APIs.
2. Deliver the React workspace, focused navigation, common controls and local Plotly.
3. Validate complete workflows, concurrency, faults, drafts and controller contracts.
4. Package and launch the browser application; retain native compatibility through
   an optional feature; document current behavior and run all gates.

## Capability evidence

Browser journeys use real Axum/Rust services and disposable projects. `workspace`,
`parity`, `tuning`, `hardware` and `production` below refer to Playwright suites in
`frontend/e2e/`. Algorithm and API evidence refers to actual Rust tests.

| Native capability | Rust service / API | Browser interaction | Validation |
| --- | --- | --- | --- |
| Project create/select/existing TOML/save | `ProjectService`, revision-checked `/config`, TOML parse/download | Project picker; settings upload/download; shared draft/save bar | Creation visually observed; `workspace` save/reload/stale revision; `parity` TOML upload, saved download, rename and retained references |
| OPC UA/watchdog/all timing/read-write tests | Communication/watchdog; engineering `io_read`, `io_write`, `watchdog_test` | Results beside configuration tests: persistent red failures with reasons, running states, per-node readings and write confirmation | `hardware`: actual OPC UA plant read/write/watchdog and runtime; partial node failures, failed writes/watchdog, retained errors through edits/save/refresh/retry, independent results, request rejection and project isolation; communication 100 ms, I/O 200 ms, independent logic 150 ms |
| Full MV/CV/DV and typed I/O/bounds/flags/external bands | `ProjectConfig` derivation, validation and normalization | Dense tables, Node IDs, typed initial values, catalog add/remove/rename | Dense 3 MV/3 CV/2 DV visual audit; `parity` rename; `workspace` optional offsets/external edges; Rust bool, band fallback and active static-I/O tests |
| Model dimensions/order/A/B/C/D/import/preview | Saved continuous model normalization/validation | Matrix editors, CSV/text upload, signal order and equations | `parity` resize, invalid import, valid import/save/reload; 3-state/5-input desktop/tablet visual audit; Rust shape/order tests |
| P/MPC/custom and manual L1/L2 | `logic`, `linear_mpc`, engineering compatibility catalog | Strategy selector, P pair table, numeric/boolean inputs | `workspace` P/custom/MPC real simulation and boolean online changes; Rust strict I/O compatibility, L1/L2 and observer continuity tests |
| Custom templates/source/build/schema/history/logs | Shared `strategy_service`, revision-checked source routes, build job | Project-owned source editor, external editor path, save/build output/source download; regeneration backups | `workspace` real Cargo build, f64/bool schema, source across navigation/projects, input history/user logs, regeneration and compiler failure; Rust external-editor conflict and backup/symlink tests |
| Simulation speed/initial values/disturbances/noise/live A/B | `RuntimeManager`, existing `SimulationCore`, scoped live config | Simulation settings, speed and scaling matrices; live apply | `workspace` lifecycle/close/reconnect/parameters; `parity` live speed/noise/A/B apply and persistence; Rust initial-state/offset/band/RK4 tests |
| Control tests/input-parameter sweeps/timing/output reports | `control_function_tests`, persisted engineering jobs | Sweep selection/ranges/count; downloadable report | `workspace` job across navigation; `parity` selected 3-point sweep plus baseline, full report download and compact list; Rust fallback/output validation/timing tests |
| Kalman upload/observer aliases/replay/persistence/charts | `kalman_tuning::import_csv_bytes`, replay job and observer schema | CSV upload, observer values, result plots and saved parameters | `tuning` real CSV replay/charts/save/reload; Rust custom aliases, input validation and replay series tests |
| Offline optimization/progress/curves/apply/cancel | Existing optimization algorithm, cancellable persisted jobs, revision-checked apply | Range/objective/scenario editors, expandable progress/curves, apply/download | `tuning` actual optimization curves and saved best values; `parity` cancellation/unlock/no partial apply; Rust scoring and fixed-value tests |
| Online optimization/objectives/safety/restore/edit locks | `online_optimization`, `RuntimeManager`, scoped objective/command APIs | Numeric/CV/MV selection, safety bounds, window/search, start/stop/save best | `tuning` manual stop restores originals and unlocks; `parity` rejects wider limits, saves tightened objectives, normal completion applies/persists best; Rust safety/execution/manual restoration before next calculation |
| Runtime lifecycle/metrics/health/active parameters | `RuntimeManager`, execution ID and expected-value guards, SSE/polling | Shared execution bar, parameters and diagnostics | `hardware` real runtime cycles/outputs/watchdog; `workspace` browser-independent simulation; `parity` concurrent value conflict retains draft; Rust previous-run/project/type/optimization locks and fault transitions |
| Plotly live/history/custom/order/windows/futures | Bounded SQLite window, persisted `PlotConfig`, local Plotly asset | MV/CV/DV groups, order controls, multi-trace builder, windows/forecasts | `workspace` zoom retained after live updates; `parity` custom plot/window persistence; light/dark visual audit; Rust extrema/boolean/latest sampling test; TS forecast timestamp/step/anchor tests |
| JSONL events/custom logs/diagnostics | Bounded JSONL tail and structured runtime/simulation logging | Log source/search/details; diagnostic snapshots | `workspace` custom history and user log through real custom simulation and log search; Rust structured logging/optimization/restoration tests; timed simulation buffer flush |
| Deployment readiness/local run/Windows commands | Saved readiness, `deployment`, config download | Validate/run/verify, edit links, optional requirements, Copy/Download | `workspace` P readiness without optional model/tuning; `hardware` explicit start/status verification; `parity` clipboard and command download; Rust command generation tests |
| Init-project/headless plant/browser feedback | CLI/shared PT1 plant; standalone `/api/virtual-hardware` | Local Plotly plant charts/manual values/node addresses | `hardware` manual apply and continued plant execution after tab close; in-app light/dark inspection; final package init-project and `--no-web` listener smoke |

Native messages were audited beyond the goal's list: variable rename, custom
source/artifact metadata, timing measurement enablement, observer help, model
imports, all eight speed modes, all optimization settings, report access and
plot-opening actions have corresponding browser workflows. Native Start web/Open
plots actions become the normal served application and embedded charts.

## Verified acceptance (2026-10-04)

- `cargo fmt`, `cargo check --offline`, `cargo test --offline`: 105 default-build
  Rust tests passed, including the real local OPC UA adapter test. Optional native
  `cargo check --offline --features legacy-native` and `cargo test --offline
  --features legacy-native`: 119 tests passed. Default `cargo tree --edges normal`
  contains no iced, eframe/egui, rfd or wgpu dependencies.
- Frontend typecheck, production build and four Vitest tests passed. All 14 main
  Playwright journeys passed against managed Rust/OPC UA servers. The separate
  production journey also passed against the final release package with external
  browser network requests blocked (15 verified browser journeys total).
- The host release package was built with `scripts/package-web.py`, initialized a
  project and launched from `/private/tmp`, outside the repository. Adjacent `ui/`
  assets, refreshed SPA deep links, local JS/CSS/Plotly, JSON API 404, static 404 and
  all six SHA-256 manifest entries verified. Packaged `virtual-hardware --no-web`
  opened its OPC UA TCP listener without a browser or HTTP feedback server.
- In-app browser: dense I/O and keyboard Tab focus, unsaved offset feedback,
  three-state A/B/C/D, 768×1024 tablet navigation and no body overflow, 1280-wide
  light/dark workspace, live three-CV Plotly, and standalone plant feedback after
  manual values. Stopping/restarting the disposable server displayed stale-data
  errors and automatically resynchronized without losing recorded charts. The
  final workspace screenshot is recorded during validation.
- Controller invariants remain covered by Rust tests: strict I/O contracts; active
  logic/static parameters; external per-edge bands and legacy defaults; CV LL/HL
  as optimization safety; L2 default/boolean L1 excluded from numeric tuning;
  squared tuning scores; structured logs; restoration before next calculation;
  fault transitions/Off suppression; separate communication/I/O/logic periods.
- All test projects and plants are isolated under `/private/tmp`; no unrelated
  live controller received test writes. Original user edits are preserved.

## Operating and platform limits

Hardware feedback was verified again on 2026-10-09: formatting, default Rust
check and 105 tests, frontend typecheck/build and four Vitest tests passed. All
four `hardware` browser journeys passed against isolated Rust/OPC UA processes,
including errors retained through edits, saves, navigation, refresh and pending
retries, recovery on success, partial read failures and project isolation. The
red watchdog failure and its short reason were also inspected in the browser.

Validation was performed on macOS. The package script builds the current host;
Linux/Windows executables and their platform dependencies need native-host build
and runtime verification. Windows service commands remain reference generation:
Service Control Manager lifecycle was not implemented in the original backend and
is not claimed as installed or functioning. Native compatibility is compiled and
tested, but a Windows/Linux native window was not opened in this environment.

Custom Rust compilation requires Cargo and available crate dependencies. Browser
source editing is confined to the selected project; built artifacts and existing
runtime project contracts retain their configured paths. Active jobs survive
browser closure and persist completed reports; jobs interrupted by backend
shutdown are not automatically resumed. SQLite stores complete samples while
browser windows intentionally downsample large results. These limits are visible
in documentation and do not turn optional engineering work into deployment gates.
