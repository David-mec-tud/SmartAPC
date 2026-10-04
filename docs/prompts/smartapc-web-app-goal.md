# SmartAPC web application migration goal

Use `$smartapc-web-migration` and `$smartapc-web-ui` to complete the following goal
in the current SmartAPC repository. If the skills are not automatically available,
read `skills/smartapc-web-migration/SKILL.md` and
`skills/smartapc-web-ui/SKILL.md` directly, following their relevant references.

## Objective

Migrate the complete current SmartAPC GUI to a polished, modern browser application
while keeping Rust as the only backend and retaining Plotly for charts. Implement
the migration, redesigned navigation, deployment workflow, documentation and
validation until the application is complete and usable. A plan, static mockup,
dashboard shell, or partial replacement does not satisfy this goal.

I authorize a fundamental redesign of the UI, page structure and navigation. You
may reorganize the current setup/deployment task sequence into focused sections,
submenus and task-specific workflows. Preserve product capabilities and controller
semantics; the old desktop layout is not a design constraint.

## Starting point and working tree

Read the current `AGENTS.md`, `README.md`, Cargo configuration, desktop handlers,
Axum router and relevant domain services. Inspect the actual working tree before
editing. My existing uncommitted changes are part of the current version to migrate:
preserve them, including recent controller, parameter, CV-band and test changes.
Do not reset the repository or replace it with a scaffold.

Create and maintain a concise migration coverage document mapping each existing
capability to its Rust service/API, browser interaction and validation evidence.
Record a short implementation plan, then execute it; do not stop after planning.
Make routine implementation and design decisions independently. Ask only when a
missing decision materially blocks the work or an action needs authorization.

## Required technology and operating model

- Frontend: React, TypeScript, Vite, Tailwind CSS, shadcn/ui, TanStack Query and
  browser routing. Use a clear component structure and one consistent design system.
- Charts: keep Plotly.js. Preserve the current plotting capabilities and integrate
  them into React components with the new application's theme.
- Backend: keep Rust, Axum, Tokio, OPC UA, existing controllers, simulation/tuning
  modules, SQLite history, TOML projects and structured logging.
- Use free, open-source packages. No required SaaS, paid component library,
  mandatory external account, cloud database or Grafana dependency. Optional
  documentation/design tools must not be required to build or run the application.
- The production frontend must build into local static assets served by Axum from
  the same origin as the API. Package scripts, fonts, icons and Plotly locally.
  Node.js is for development/building; the deployed application needs only the
  Rust backend and its packaged files.
- Support direct browser navigation and refreshed deep links while preserving real
  API error responses. Provide a documented frontend development proxy to Axum.
- The runtime, simulation and long-running backend jobs must operate independently
  of browser tabs. Reloading, closing or reconnecting the UI must not implicitly
  stop them or affect controller cycle times.

## Functional migration scope

Move desktop-only orchestration into reusable Rust application services and expose
it through real, typed APIs. Reuse the current algorithms and validations. Include:

1. Project creation, selection and existing project loading/saving.
2. OPC UA communication, watchdog and all timing settings; existing read/write and
   watchdog test interactions.
3. MV/CV/DV catalog and complete I/O definition, typed values, Node IDs, initial
   values, bounds, enable/setpoint inputs and optional external CV bands.
4. Model definition, A/B/C/D import/editing, state dimensions, signal ordering,
   previews and the current optional/required model semantics.
5. Compatible P-controller, linear MPC and custom Rust strategy selection and
   parameterization, including per-MV L1/L2 movement-cost selection.
6. Custom Rust template generation, source-editing workflow, build/check output,
   parameter schema, compatibility checks, history context and user log helpers.
7. Simulation setup and execution, speed, initial values, disturbances, noise,
   live A/B scaling and progress/results.
8. Control function tests, parameter/input sweeps, timing metrics, output validation
   and reports.
9. Kalman CSV upload/import, observer parameter aliases, replay, persistence and
   result charts.
10. Offline parameter optimization, progress, result curves and applying results.
11. Online parameter optimization, objectives, duration, safety limits, status,
    start/stop and the existing restoration and manual-edit-locking semantics.
12. Runtime start/stop, execution state and metrics, active online parameters,
    watchdog health, logs, custom user logs and diagnostics.
13. Plotly live/history/custom views, selectable MV/CV/DV series, persisted plot
    settings, sliding windows, future MPC predictions, Kalman and optimization plots.
14. Deployment readiness, supported local run/Windows command-generation workflows,
    project initialization and virtual hardware.

This list is a minimum: inspect native actions for capabilities it does not name.
Do not silently drop a feature, replace it with a placeholder or claim a mocked
workflow is functional. Use browser uploads/downloads where native file dialogs
were used. Show meaningful progress/results/errors for long-running jobs and
cancellation where the workflow supports it.

Keep frontend types aligned with Rust DTOs. The backend owns compatibility,
validation, normalization and action availability. Ensure parameter mutations work
correctly for the active simulation as well as runtime. Handle editing-project
selection separately from the backend's active execution project and prevent stale
or mismatched updates across projects or concurrent clients.

## UI and workflow redesign

Design a polished engineering workspace with readable typography, calm surfaces,
clear hierarchy, restrained colors, shared spacing and components, and consistent
light/dark themes. Give charts, I/O tables and matrix editors enough useful space.
Use text and icons alongside state colors. Avoid a collection of generic oversized
cards or a one-to-one copy of the old desktop task list.

A reasonable navigation starting point is:

- **Overview**: active execution, project, health, recent events and useful trends.
- **Project configuration**: communication/watchdog/timing, variables and I/O,
  model, strategy and project settings.
- **Engineering**: simulation, control function tests, Kalman tuning and offline
  parameter optimization, organized as focused submenus or pages.
- **Operation**: live trends, online parameters, online optimization, logs/events
  and diagnostics.
- **Deployment**: readiness, saved configuration, supported local/service run
  commands and execution verification.

Improve this grouping when actual workflows justify it. Keep project selection,
runtime/simulation status, connection status and primary operational actions
consistent throughout the app. Distinguish saved configuration, unsaved drafts,
applied online values, job results and stale/unavailable data.

Refactor deployment into a short, meaningful flow: validate the saved project,
show real readiness issues with links to their editors, present the supported run
commands, and verify status after an explicit run/start action. Separate required,
optional and not-applicable items. Simulation, Kalman tuning and parameter
optimization must not become universal deployment gates. Require models and other
configuration only when the selected strategy or existing backend rules require them.

The current deployment page is a placeholder and the backend generates Windows
service commands. Provide accurate support/platform status and usable Copy/Download
actions; generating commands is not proof of an installed or functioning service.
Keep deployment local/self-hosted. Do not execute privileged installation/removal
commands or publish to an external host as a side effect of this migration.

## Runtime and domain contracts

Preserve all current `AGENTS.md` invariants and validate them on the backend:

- Strict strategy compatibility with the current I/O contract and custom schemas.
- Online editing limited to active logic parameters and eligible static I/O values.
- Optional external `cv.<name>.band_low/high` inputs override local tuning per edge;
  exclude external edges from local editing and optimization. Preserve initial
  offsets, blank-value legacy defaults and offline/fallback behavior.
- CV LL/HL remain absolute online-optimization safety boundaries, not MPC constraints.
- Default MV movement cost remains L2. `mpc.mv.<name>.use_l1` remains a boolean
  manual/online choice, excluded from numeric optimization; scoring stays squared.
- Manual, safety and execution stops restore original parameters before the next
  controller calculation. Normal completion/convergence applies best-known values.
- Preserve runtime fault-state behavior and the rule that Off writes no outputs.
- Communication/watchdog use `communication.cycle_ms`; I/O uses `io.cycle_ms`, a
  multiple of the communication period; controller/simulation/tuning use independent
  `logic.cycle_ms`. Browser update rate must not redefine any of these periods.
- Keep runtime/simulation logs and optimization events structured and machine-parseable.

Run expensive builds, sweeps, replay and optimization away from controller timing
tasks. Prefer HTTP commands plus SSE for status/job notifications where suitable,
with reconnect/resynchronization and a bounded polling fallback. Keep chart windows
and rendering bounded, preserve zoom/legend state, and keep predictions correctly
aligned with historical data.

## Delivery and acceptance

Work in complete vertical slices, then close every capability in the migration
coverage document. Keep legacy native behavior available during the transition;
once parity is demonstrated, make the standard Rust build independent of iced,
eframe/egui and native file dialogs through optional legacy features or focused
retirement. Keep the `virtual-hardware` OPC UA test plant runnable headlessly;
replace its useful native plot feedback with a browser Plotly view when needed.

Provide reproducible development, production-build and launch commands, lockfiles,
and an asset-packaging approach that works from the documented launch location.
Inspect actual build/CI/release scripts before adjusting them. Package the new
frontend and remove obsolete native build requirements where appropriate; preserve
checks for still-supported helper or legacy features.

Run `cargo fmt`, `cargo check`, `cargo test`, frontend type checking, production
build and meaningful frontend tests. Use Playwright or equivalent browser automation
for critical journeys against the actual Rust backend. Visually inspect the real UI
using the available browser skill, including dense tables/matrices, Plotly views,
light/dark themes, practical viewport sizes, errors, unsaved changes and reconnects.

Use temporary test projects and local virtual hardware to verify configuration
save/reload, strategy compatibility, simulation start/stop, active parameter editing,
live plots, job progress/results and deployment readiness with optional work absent.
Validate production asset serving, deep links, offline operation and continued
backend execution while no browser is connected. Do not run smoke-test writes
against an unrelated live controller.

Update README, AGENTS and relevant API/build/run documentation to match the finished
product. Record limitations and unverified platform behavior honestly. The goal is
complete only when the main application works fully through the browser, existing
projects remain usable, domain behavior is preserved and the required checks pass.
If an external blocker prevents completion or validation, preserve progress and
report the blocker and remaining work without presenting partial work as complete.
Ask whether I want to commit the finished changes; do not commit or rewrite history
automatically.
