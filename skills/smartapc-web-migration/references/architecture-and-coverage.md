# Architecture and feature coverage

Starting point inspected on 2026-10-04. Reinspect the active working tree before
using this map; it includes user changes that may not yet be committed.

## Source map

| Source | Relevant behavior |
| --- | --- |
| `src/desktop.rs` | Native iced editors, action orchestration, strategy generation/builds, native file import, engineering views |
| `src/web.rs` | Axum router, basic project/runtime/simulation APIs, inline HTML/JavaScript Plotly and Kalman views |
| `src/main.rs` | `serve`, `desktop`, `init-project`, and `virtual-hardware` commands; default command is `serve` |
| `src/project.rs` | Project paths, loading/saving TOML, parameter and plot persistence |
| `src/project_config.rs` | Configuration types, normalization, strategy compatibility, setup-task metadata, parameter availability |
| `src/runtime.rs` | Runtime manager, runtime/simulation ownership, active execution parameters, online optimization |
| `src/simulation.rs`, `src/model.rs` | Model execution, simulated plant, disturbances and scaling |
| `src/kalman_tuning.rs` | CSV import, observer replay, parameter aliases, results store |
| `src/parameter_optimization.rs` | Offline optimization, progress, scoring, result application |
| `src/online_optimization.rs` | Online optimization session, safety stops and parameter restoration |
| `src/control_function_tests.rs` | Input/parameter sweeps, output checks, timing reports |
| `src/logic.rs`, `src/linear_mpc.rs` | Controller/strategy contracts, dynamic custom logic, MPC and observer behavior |
| `src/storage.rs` | SQLite signal history and series catalog |
| `src/deployment.rs` | Windows service command generation, not proof of installed service support |
| `src/virtual_hardware.rs` | Separate local OPC UA PT1 plant and an eframe/egui plot window |

The initial API already covers project configuration and plot configuration,
runtime/simulation start and stop, execution snapshots, read parameters, runtime
parameter writes, history windows and series catalogs, Kalman result reads, a control
step helper, and deployment command generation. Check request/response semantics
rather than assuming that an existing route covers the desktop workflow fully.

At this baseline, `/api/execution/params` is read-only while writes use
`/api/runtime/params`; a unified web UI must provide an explicit, correct path for
editing the active simulation as well as runtime. The current configuration update
handler returns its input even though persistence normalizes configuration; refetch
or return authoritative saved values when building the new frontend.

## Coverage inventory

Record implementation and validation for these capabilities:

1. Project list, creation, selection, existing TOML load/save and plot settings.
2. OPC UA endpoint, communication timing, watchdog modes, watchdog/I/O tests.
3. MV/CV/DV catalog; typed I/O definition, Node IDs, static initial values, limits,
   enables, setpoints and optional external CV band inputs.
4. Optional continuous model, A/B/C/D import/editing, state count, signal ordering
   and previews; preserve the selected strategy's actual model requirements.
5. Compatible strategy selection, P-controller/MPC parameters, per-MV L1/L2 choice.
6. Custom Rust template generation, source-editing workflow, build/check results,
   parameter schema, strict I/O compatibility and user log helpers. Preserve use of
   an external editor; if editing is offered in the browser, use project-scoped
   source APIs and an appropriate editor rather than arbitrary host-file access.
7. Simulation setup and execution: speed, initial values, disturbances, noise,
   live A/B scaling, progress, stop and parameter application.
8. Control function tests: sweep configuration, execution, output validation,
   timing/report details and case previews.
9. Kalman CSV upload/import, aliases, parameter editing/persistence, observer replay,
   errors and Plotly results. Preserve existing CSV semantics and validation.
10. Offline parameter optimization: configuration, progress, report, result curves
    and explicit application of selected/best results.
11. Online optimization: active execution, objective weights, duration, tightened CV
    limits, start/stop, status, best/original restoration and manual-edit locking.
12. Runtime lifecycle, runtime/simulation exclusion, metrics, watchdog status,
    active online parameters and accurate backend health.
13. Plotly live/history/custom plots, MV/CV/DV grouping, future MPC predictions,
    Kalman replay and optimization result charts; user zoom and persisted settings.
14. Structured logs, custom strategy user logs, optimization events and diagnostics.
15. Deployment readiness and supported local/Windows command generation with
    working documentation rather than a fabricated installed-service status.
16. Project initialization and the separate virtual-hardware CLI, including useful
    browser Plotly feedback for its signals when replacing a native plot window.

Trace helpers such as `save_*_task`, `test_io_*_task`,
`generate_logic_template_task`, `build_custom_logic_task`, `run_observer_task`,
`run_control_function_tests_task`, and optimization handlers out of `desktop.rs`.
Reuse their Rust behavior without exposing iced UI state as an API contract.

## Contracts to carry through migration

Read the current AGENTS rules for full detail. These points are particularly easy
to lose when changing UI and API boundaries:

- Runtime states and fault progression keep their existing behavior; Off writes no
  outputs. UI navigation and connectivity cannot change controller timing.
- Communication/watchdog use `communication.cycle_ms`; I/O uses `io.cycle_ms`, a
  multiple of the communication period; logic, simulation and tuning use
  `logic.cycle_ms`, independently.
- Optional `cv.<name>.band_low/high` OPC UA inputs override local tuning per edge;
  external edges are absent from local editing and optimization. Initial offsets
  and legacy blank/deadband behavior remain valid for offline execution and fallback.
- CV LL/HL are absolute online-optimization safety boundaries, not MPC prediction
  constraints. Optimization limits may be tightened, not loosened beyond them.
- `mpc.mv.<name>.use_l1` stays boolean and manually/online selectable. L2 is default;
  numeric optimization does not change it, and scoring retains squared movement.
- Manual, safety and execution stops restore original parameters before the next
  controller calculation; normal convergence/completion uses best-known parameters.
- Only the active strategy's parameters and eligible static I/O values are editable
  online. Existing strict strategy/I/O compatibility and typed schemas remain intact.
- Preserve machine-parseable runtime/simulation logs and optimization events.

## Integration choices

Keep history in SQLite and projects in TOML unless a verified migration requirement
justifies a compatible change. Preserve existing projects without manual rebuilding.

Use HTTP requests for user commands. SSE is the preferred simple transport for
state, job progress and live notifications; bounded history endpoints can supply
chart windows, with polling fallback if appropriate. Document identities, reconnect
resynchronization and error semantics. Do not stream or render every control cycle
merely because the backend computes at that rate.

Replace client-native file paths with uploads/downloads or deliberate backend-host
paths. Keep expensive jobs off the controller task and make saved state, session
state, active execution state and completed job results explicit.

Production assets can be embedded or packaged beside the Rust executable. Choose
one approach and test it from the documented launch location. Reuse the existing
Plotly asset where practical. Check direct deep links, `/api` errors, asset lookup,
local offline operation and browser-independent execution.
