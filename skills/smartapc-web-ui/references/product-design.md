# SmartAPC product design

Use this reference when designing navigation, the application shell, deployment
readiness, or shared operational components. The grouping below is a starting
point, not a requirement to invent a page for every label.

## Information architecture

| Area | Contents | Purpose |
| --- | --- | --- |
| Overview | Active project, connection, execution, recent events, useful trends | See what is happening and reach the next action |
| Project configuration | Communication, watchdog, timing, MV/CV/DV catalog, I/O definition, model, control strategy | Define the controller and its hardware contract |
| Engineering | Simulation, control function tests, Kalman tuning, offline parameter optimization | Evaluate and improve a configuration |
| Deployment | Backend readiness, saved configuration, supported run/service commands, verification | Prepare and run a configured project on a target host |
| Operation | Live trends, online parameters, online optimization, structured logs and diagnostics | Observe and operate the active runtime or simulation |

Use nested navigation, tabs, or submenus where they make related work easier to
find. Keep the main navigation compact. Project creation and selection belong in a
consistent location in the shell. The user permits changing names, page boundaries,
and the old setup sequence.

## Deployment flow

The current desktop sidebar mixes configuration and engineering tasks, and its
deployment page is a placeholder. A short deployment flow should summarize actual
backend readiness, reveal real configuration errors, link to the editor that fixes
each error, show the supported run commands, and help verify execution afterwards.

Distinguish **required**, **optional**, and **not applicable**. Simulation, Kalman
tuning, offline optimization, and online optimization are not universal deployment
prerequisites. A model is required when the chosen strategy actually requires it;
watchdog requirements depend on the configured mode. Backend validation and strict
strategy compatibility remain authoritative. Do not infer readiness from menu
position, a nonempty service name, or the old `SidebarTask.configured` flags.

Separate saved configuration readiness from hardware reachability and from an
already running controller's health. A live hardware test belongs to an explicit
user action; opening a readiness page must not write OPC UA values.

The existing backend generates Windows `sc.exe` commands. Preserve supported
command generation with Copy or Download actions and accurate platform/support
status. Generating a command does not prove that a Windows service has been
installed or that the executable implements the service lifecycle. Avoid turning
this UI migration into a new cloud hosting or privileged service-management product.

## Visual direction

Aim for a polished engineering workspace: strong hierarchy, calm neutral surfaces,
readable typography, restrained accent colors, and enough density for signal tables
and matrices. Share typography, spacing, radii, borders, focus styles, and semantic
colors through tokens. Use consistent light and dark themes, including Plotly.

Keep large chart and matrix workspaces useful. Avoid placing every field in a
separate oversized card, decorative gradients competing with data, or excessive
animation during live operation. Use local fonts or system fonts and bundled icons.
Show state with text and symbols as well as color, especially Running, Hold, Alarm,
Off, simulation activity, and disconnected/stale data.

Choose the final palette and layout from the actual product; these instructions
deliberately leave room for a fundamental redesign.

## Important interactions

- Keep Start, Stop, Save configuration, and Apply online unmistakable. Runtime and
  simulation controls must reflect backend mutual-exclusion rules.
- Show the active project when editing a different project. Never apply an editing
  project's draft parameters to another project's running execution.
- Show why actions are unavailable, including incompatible strategy, active
  execution locks, or an optimization that locks manual parameter editing.
- Keep communication, I/O exchange, and logic cycle settings separate and explain
  their effects in user terms. Do not let browser refresh cadence change them.
- Use grouped, searchable parameter tables and practical MV/CV/DV editors. Preserve
  numeric precision, units, optional Node IDs, typed values, and meaningful defaults.
- Preserve entered forms across ordinary failures and handle unsaved navigation
  explicitly. Backend validation messages should point to the relevant field.
- Keep job results discoverable after navigating away. Provide reconnect/resync and
  explicit data-age indicators when the server becomes unavailable.
- Replace native file dialogs with browser upload/download interactions while
  making it clear whether a file belongs to the user's machine or the backend host.

## Visual and interaction checks

Inspect a real configuration editor, a dense matrix or I/O table, a live Plotly
view, and deployment readiness with optional items absent. Test light/dark themes,
keyboard focus, project switching, save failures, stale data, and job progress.
Use the browser skill for real screenshots and clicks; do not mark screens verified
from component source alone.
