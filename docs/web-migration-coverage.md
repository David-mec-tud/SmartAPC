# Browser migration coverage

The source of truth is `docs/prompts/smartapc-web-app-goal.md`, the current
`AGENTS.md`, and the native actions in `src/desktop.rs`. Existing uncommitted
controller, CV-band, parameter and test changes are part of the baseline.

## Implementation sequence

1. Extract reusable engineering services, typed job APIs, saved-project validation,
   configuration locks and project-scoped execution mutations.
2. Build one React workspace with focused configuration, engineering, operation and
   deployment pages, shared components, local Plotly, and light/dark themes.
3. Complete browser journeys against real Rust services, including long-running
   work, uploads, parameter edits, persisted plots and error/reconnect behavior.
4. Validate parity, package assets, gate legacy native dependencies, document and
   verify development/production/headless launch flows, and run all quality gates.

## Capability evidence

`Pending` means implementation or sufficient validation is still required. Source
inspection and passing builds alone do not prove browser parity.

| Native capability | Rust service / API | Browser interaction | Validation |
| --- | --- | --- | --- |
| Projects: create/select/TOML/save | `ProjectService`, `/api/projects`, revision-checked `/config`, TOML parse/download | Shell project picker, configuration/settings | Browser creation observed; Playwright save/reload/stale-draft journey; TOML upload/download journey pending |
| Communication/watchdog/timing and read/write tests | `communication`, `watchdog`, engineering `io_read`/`io_write`/`watchdog_test` jobs | Configuration/communication and I/O | Forms implemented; real virtual-plant test journey pending |
| MV/CV/DV, complete typed I/O, external bands | `ProjectConfig`, I/O derivation and normalization | Configuration/I/O, typed initial values | Dense CV editor visually inspected; Playwright optional-offset and MPC external-edge journeys; broader catalog journeys pending |
| Model dimensions/order/A/B/C/D/import/preview | `ProjectConfig` normalization | Configuration/model matrix editors/import | 3-state, 5-input matrices and ordering inspected; upload/resize/error journeys pending |
| P/MPC/custom strategy, strict contracts, L1/L2 | `logic`, `linear_mpc`, engineering catalog | Configuration/strategy, operation/parameters | Rust compatibility/movement tests; Playwright boolean MPC simulation editing and excluded external edges; custom browser journey pending |
| Custom template/source/build/schema/history/log helpers | Shared `strategy_service`, project source API and build jobs; native handlers reuse service | Configuration/strategy, full build output and source download | Extracted original templates; full custom build/logging and source-draft journey pending |
| Simulation: speed/initials/noise/disturbance/live scaling | `RuntimeManager`, `SimulationCore`, scoped live configuration API | Engineering/simulation | Playwright lifecycle, applied parameters, reconnect and continued execution with no browser; live noise/scaling/initials journey pending |
| Control tests: sweeps/timing/validation/report | `control_function_tests`, persisted engineering jobs | Engineering/tests, sweep table, reports/download | Rust job persistence/restart-read test and Playwright navigation/result journey; selected sweep/report download journey pending |
| Kalman upload/aliases/replay/save/charts | `kalman_tuning::import_csv_bytes`, replay job, existing observer/alias validation | Engineering/kalman and job result Plotly charts | Rust CSV/alias/replay tests pass; browser upload/save/replay journey pending |
| Offline optimization/progress/curves/apply/cancel | `parameter_optimization`, progress forwarding and cancellable jobs, revision-checked apply | Engineering/optimization, reports/curves | Existing Rust algorithm tests pass; browser progress/cancel/apply journey pending |
| Online optimization/objectives/safety/restoration/locks | `online_optimization`, `RuntimeManager`, scoped objective-save and command APIs | Operation/optimization, numeric/CV/MV objective selection | Existing Rust restoration/safety tests pass; real browser objective/start/stop/restoration journey pending |
| Execution lifecycle/metrics/active parameters/health | `RuntimeManager`, scoped commands/parameters, SSE/poll snapshots | Shared status bar, operation parameters/diagnostics | Rust mismatched-project/type/lock tests; Playwright runtime-independent simulation and MPC parameter persistence; real OPC UA runtime pending |
| Plotly/history/custom/order/windows/predictions | `storage`, `PlotConfig`, local Plotly asset | Operation/trends, persisted groups/custom builder, forecasts | Playwright live history and zoom persistence after data update; light/dark visual inspection; custom/order/future/offline bounded-window audit pending |
| Structured logs/user logs/events/diagnostics | Bounded JSONL tail API and existing runtime/simulation logs | Operation/logs and diagnostics | Existing structured-log Rust tests pass; custom log/event browser journey pending |
| Readiness/local run/Windows commands/copy/download | `project_readiness`, `deployment`, saved TOML download | Deployment validate/run/verify, copy/download | Rust and Playwright readiness without optional model; command generation tests pass; copy/download and actual OPC UA start pending |
| Init-project/headless virtual hardware/browser feedback | CLI, `virtual_hardware` | Pending | Pending |

## Acceptance evidence still required

- Rust formatting/check/tests and frontend typecheck/build/meaningful tests.
- Real browser journeys and visual checks: dense I/O/matrices, plots, themes,
  keyboard, tablet/desktop, drafts, project switching, errors and reconnects.
- Temporary projects and local OPC UA test plant; no unrelated controller writes.
- Same-origin static assets, refreshed deep links, JSON API errors, offline assets,
  bounded charts, and browser-independent runtime/simulation/jobs.
- Domain invariants in the goal and AGENTS, including timing ownership, Off output
  suppression, strict compatibility, typed L1 selection, external bands and tuning
  restoration before the next calculation.
- Native feature checks after demonstrated parity, production packaging, launch
  documentation and honest Windows platform/service support status.

## Verified checkpoint (2026-10-04)

- `cargo fmt`, `cargo check --offline`, and `cargo test --offline`: 115 Rust tests
  passed, including the real localhost OPC UA adapter integration test. The latter
  requires listener permission outside the restricted sandbox.
- Frontend typecheck, production build, one meaningful optional-field interaction
  test, and four Playwright journeys against real Axum passed. Sources:
  `frontend/e2e/workspace.spec.ts` and `frontend/src/components/ui.test.tsx`.
- In-app browser inspection: project creation, dense I/O (3 MV, 3 CV, 2 DV), CV
  offset save, 3-state A/B/C/D ordering, simulation start/stop and live Plotly.
  Light/dark themes inspected. Responsive DOM geometry at width 768 showed body
  and main both width 768 with the compact navigation visible.
- All smoke projects are isolated under `/private/tmp`. No existing project or
  external controller received smoke-test writes. Initial user changes remain
  outside the migration commit.

This checkpoint does **not** close the migration: full workflow journeys, remaining
draft/ownership edge cases, bounded history/forecast checks, headless virtual
hardware, production packaging and native dependency retirement remain required.
