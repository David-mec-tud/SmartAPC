---
name: smartapc-web-migration
description: "Migrate SmartAPC native GUI workflows into a complete React browser application served by the existing Rust/Axum backend. Use for feature coverage, extracting desktop orchestration into Rust services, extending typed APIs and background jobs, preserving controller contracts, and changing the local deployment flow."
---

# SmartAPC Web Migration

Migrate the current working version of SmartAPC to a complete browser application
with Rust as the only backend. Pair with `$smartapc-web-ui` for frontend design.
Completion means working product flows, not a dashboard shell or an API inventory.

## Establish the actual baseline

Read the checkout's current `AGENTS.md`, `README.md`, and relevant implementation.
Inspect `git status` before edits; existing uncommitted changes are part of the
requested current version. Do not reset, replace, or omit them to simplify migration.

Read [Architecture and feature coverage](references/architecture-and-coverage.md)
when beginning a full migration or extracting a native-only workflow. Its source
map identifies an inspected starting point; verify names and behavior against the
current tree. Maintain a concise coverage record linking each native capability to
its Rust service/API, web interaction, and actual validation evidence.

## Target architecture

- Frontend: React, TypeScript, Vite, Tailwind CSS, shadcn/ui, TanStack Query, browser
  routing, and Plotly. Use free, open-source packages; no required cloud or paid plugin.
- Backend: existing Axum/Tokio runtime, OPC UA, watchdog, strategy logic, simulation,
  tuning, SQLite, TOML persistence, and structured logging.
- Production: build the frontend into static assets and serve them with Axum under
  the same origin as `/api`. Package all assets locally and support SPA deep links
  without turning API errors into HTML responses. Node is a development/build tool.
- The backend runs without a native window or a browser connection. Browser refresh,
  tab closure, chart rendering, and transport reconnect do not own control cycles.
- Preserve native-only helpers during migration. Once web parity is demonstrated,
  remove native UI dependencies from the normal backend build through optional
  features or focused retirement. Preserve the virtual-hardware CLI and test plant;
  adapt or feature-gate its native plot window so headless use does not require egui.

## Move orchestration into Rust

Extract behavior currently tied to `iced::Task`, native dialogs, and desktop editor
state into reusable Rust application services. Keep algorithmic modules intact
unless migration requires a specific, verified correction. Axum handlers call these
services; React sends typed requests and renders their results.

Expose every required operation through real APIs, including missing engineering
workflows. Run strategy builds, replay, sweeps, and optimization away from controller
timing tasks; expose progress and terminal success/error states. Give results explicit
project/job identities and cancellation where supported by the workflow. A browser
disconnect alone is not a cancellation signal.

Use typed request/response DTOs and keep frontend types aligned through a documented
contract, generation, or meaningful contract tests. Make runtime versus simulation
parameter application explicit; avoid blindly reusing runtime-only mutation routes
for the unified execution UI. Return normalized, saved values where appropriate.

## Preserve domain behavior

The current `AGENTS.md` is authoritative for strict strategy compatibility, active
online parameters, external CV bands, CV safety limits, movement costs, optimization
restoration, structured events, and timing ownership. Enforce these rules in Rust
even when the frontend also validates a form. Preserve generated strategy contracts,
custom parameter types, history context, user logging, and existing project files.

Different browser clients or stale drafts must not bypass configuration locks,
optimization locks, runtime/simulation exclusion, or parameter ownership. Keep
existing normalization behavior when configuration changes affect online parameters.

## Navigation and deployment

Reorganize configuration, engineering, operation, and deployment into focused
sections and submenus. Deployment readiness must reflect backend requirements for
the selected strategy. Optional simulation/tuning work must not become a universal
gate. Preserve supported local run and Windows command-generation behavior with
accurate status; execute only actions within the user's actual task authorization.

## Validate and finish

Build complete vertical slices and then close the feature coverage record. Validate
existing projects and the important safety/timing contracts with meaningful Rust
tests, plus representative browser journeys against the real backend. Run
`cargo fmt`, `cargo check`, `cargo test`, frontend type checking, production build,
and applicable frontend tests. Check a production serve flow with locally packaged
assets and headless virtual hardware. Use temporary test projects for smoke tests.

Update README, AGENTS, API/run/build documentation, and packaging instructions when
their workflows change. Report remaining gaps honestly; screenshots or a green
build alone are not feature parity. Follow the repository's commit convention and
ask about committing after the authorized work is complete.
