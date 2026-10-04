---
name: smartapc-web-ui
description: "Design, implement, or review SmartAPC's React and TypeScript browser UI with Vite, Tailwind CSS, shadcn/ui, and Plotly. Use for SmartAPC screen design, navigation, configuration forms, operational dashboards, and chart integration; pair with smartapc-web-migration when moving native functionality into backend APIs."
---

# SmartAPC Web UI

Build a coherent browser interface for SmartAPC, an industrial controller and
engineering tool. The user authorizes a fundamental visual and navigation redesign;
preserve the controller's capabilities and meaning rather than its desktop layout.

## Context and boundaries

- Work in the active SmartAPC checkout. Read its current `AGENTS.md` and relevant
  product code; do not assume that an installed skill's initial repository map is
  newer than the working tree.
- Use React, TypeScript, Vite, Tailwind CSS, and shadcn/ui. Use TanStack Query for
  server state and a browser router for pages. Reuse an established frontend's
  compatible conventions instead of scaffolding a second application.
- Keep Plotly for every existing chart workflow. Match its appearance to the UI;
  retaining Plotly does not require retaining the old HTML pages or toolbar design.
- Use free, open-source components and locally served assets. The shipped app must
  work without CDN access, an external account, or a subscription. Context7 and
  design services are optional development aids.
- OPC UA, controller calculations, timing, validation, safety behavior, simulation,
  optimization, project persistence, and strategy builds belong in Rust. Browser
  code owns presentation and editable drafts.

## Design and navigation

For a shell, navigation overhaul, or first implementation, read
[Product design](references/product-design.md). It defines task-oriented grouping
and the distinction between configuration, engineering, operation, and deployment.
Choose a clear visual direction, then express it through shared tokens and reusable
components. Do not reproduce the desktop's single setup-task list page by page.

Keep the selected editing project and the project executing in Rust distinguishable.
Display the active execution type, runtime state, connection health, and data age
consistently across pages. Navigation order and optional tuning completion do not
determine whether a controller is compatible or ready to run.

## Forms and server state

- Keep drafts separate from saved server values. Show validation next to the field;
  preserve entered data after an error. Distinguish Save, Apply online, Run, and
  explicit Stop rather than hiding their different effects behind one action.
- Scope queries and cached results to the correct project and execution. Discard
  late responses from a previously selected project. Refetch authoritative values
  after mutations; never show a controller parameter as applied before Rust accepts it.
- Treat booleans, numbers, strings, and absent values as distinct types. Preserve
  blank optional numeric fields. In particular, movement-cost selection is boolean,
  not a numeric tuning variable, and CV bands are offsets relative to the setpoint.
- Obtain editable parameters, compatibility, and action availability from backend
  responses. A disabled frontend control supplements server enforcement.
- Represent long-running jobs with progress, current status, terminal results and
  errors, and cancellation when the backend supports it. Refreshing or closing the
  browser must not implicitly stop controller execution or optimization jobs.

## Plotly integration

Create reusable chart components rather than embedding new HTML strings in Rust.
Preserve sliding history windows, MV/CV/DV selection, persisted custom plots,
multiple traces, MPC future predictions, Kalman replay, and optimization curves.
Keep forecast time and measured history distinct and retain existing time semantics.

Preserve zoom and legend selections across live updates. Resize plots with their
container and clean up Plotly resources when components unmount. Bound displayed
data and decouple rendering frequency from communication, I/O, and logic cycles.
Apply the shared theme to chart backgrounds, labels, grids, legends, and tooltips;
retain units, readable axes, and stable signal colors.

## Verification

Use the available browser-control skill for visual inspection and actual local
interactions. For meaningful behavior changes, cover representative user journeys
with frontend tests or Playwright rather than tests of CSS wording or component
implementation details. Check keyboard use, matrix/table scrolling, unsaved drafts,
project switching, errors, reconnects, and charts at practical desktop and tablet
widths. Confirm frontend type checking and production build as well as applicable
repository gates. Report observed results and unverified behavior accurately.
