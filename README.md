# SmartAPC

SmartAPC V1 is modular control software written in Rust for OT and industrial environments.

## Current Status (April 2026)

### Implemented

- Modular runtime with clear interfaces:
  - `ProtocolAdapter` (V1: OPC UA client adapter)
  - `WatchdogModule` (`none`, `counter_mirror`)
  - `ControlLogic`: production-ready `p_controller`, model-based `linear_mpc`, and project-local `custom_rust` `cdylib`
  - Persisted `linear_state_space` model for offline simulation
- Runtime state machine: `Running -> Hold -> Alarm -> Off`
  - Fault behavior: hold last value, enter alarm, then switch off
  - No outputs are written in the `Off` state
- Structured JSONL runtime and simulation logging under `projects/<project>/data/`
  - Per-cycle inputs, outputs, online parameters, simulation time, user logs, and MPC diagnostics
  - Online optimization candidate, safety-interruption, restoration, and completion events
  - Online-parameter changes
- Native `iced` desktop application:
  - OPC UA communication and watchdog setup
  - MV/CV/DV catalog and I/O definition with OPC UA read/write tests
  - Optional continuous-time state-space model with A/B/C/D import and matrix editor
  - Controller simulation with speed selection, initial values, disturbances, noise, and live A/B scaling
  - Control strategy selection, control-function tests, online parameters, and SQLite plots
- Local web API:
  - Project configuration, runtime and simulation control, execution state, parameters, plots, and Windows service commands
- Interactive Plotly web view at `GET /plots`
- Automatically follows the runtime or simulation
- Online parameter optimization is available from the desktop setup sidebar while a runtime or
  simulation is active. Its selectable objective combines weighted CV MSE terms with weighted
  integral squared delta-MV terms over a configured `HH:MM:SS` window. CV objective limits are
  initialized from predefined I/O LL/HL limits and may only be tightened. Normal completion applies
  the best-known parameters; manual stop, execution stop, invalid data, or a CV safety-limit touch
  restores the original parameters before the next controller calculation.
  - Uses locally bundled Plotly.js
  - Supports sliding windows, sortable MV/CV/DV panels, custom multi-trace plots, and MPC future predictions
- Virtual hardware:
  - Separate local OPC UA PT1 server
  - Live plot with a three-minute window

### Placeholder

- The deployment setup page is a placeholder; deployment API commands are available.

The optional continuous-time state-space model uses `x_dot = A x + B u` and `y = C x + D u`.
Offline controller simulation integrates the model with fixed-step RK4 and zero-order hold per
control cycle. The production runtime does not simulate the plant internally.

## Build, Test, and Run

### Browser workspace migration (October 2026)

The React/TypeScript workspace now runs through the Rust/Axum backend. Build its
local assets before using `serve`:

```sh
cd frontend
npm ci
npm run build
cd ..
cargo run -- serve --project-root ./projects
```

Open `http://127.0.0.1:8080`. Use Node.js 22 or newer for frontend development and
tests. The deployed frontend uses locally packaged assets and requires no Node
process or CDN. The full migration and feature-parity audit are still in progress;
native helpers and their build dependencies remain available during the transition.
See [web development, production assets and API contracts](docs/web-application.md)
and [migration coverage](docs/web-migration-coverage.md) for current evidence and
remaining work. The browser deployment page validates saved strategy requirements
and generates local/Windows commands; it does not claim installed Windows service
support. The native deployment page remains a placeholder.

```bash
cargo test
cargo run -- desktop
cargo run -- serve
cargo run -- virtual-hardware
```

Start the desktop application with a project root:

```bash
cargo run -- desktop --project-root ./projects
```

Disable the embedded web API:

```bash
cargo run -- desktop --project-root ./projects --no-web
```

## Project Layout

```text
projects/
  <project_name>/
    project.toml
    data/
      runtime.sqlite
      runtime.log
      simulation.sqlite
      simulation.log
    logic/
      custom_strategy/   # created by template generation
```

## Virtual Hardware Quick Start

1. Start virtual hardware:

```bash
cargo run -- virtual-hardware --host 127.0.0.1 --port 4855 --dt-ms 100 --tau-s 5.0 --gain 1.0 --history 120
```

2. Start the desktop application in a second terminal:

```bash
cargo run -- desktop --project-root ./projects
```

3. In the application:
   - Set the endpoint to `opc.tcp://127.0.0.1:4855/`.
   - Configure `read_node = ns=2;s=wd_counter_read` and `write_node = ns=2;s=wd_counter_mirror`.
   - Define variables and I/O, for example `cv.y1.*` and `mv.u1.*`.
   - Select a control strategy and start the runtime.

Virtual hardware OPC UA Node IDs:

- `ns=2;s=y1_value`
- `ns=2;s=y1_target`
- `ns=2;s=y1_HL`
- `ns=2;s=y1_LL`
- `ns=2;s=u1_write`
- `ns=2;s=u1_read`
- `ns=2;s=wd_counter_read`
- `ns=2;s=wd_counter_mirror`

## Control Strategies

The **Control Strategy** tab offers only strategies compatible with the current I/O definition:

- `P Controller` (`p_controller`)
- `Linear MPC` (`linear_mpc`)
- `Custom Control Strategy` (`custom_rust`)

### P Controller

- Uses explicit CV-to-MV pairings with `kp` and `bias`
- Formula: `u = kp * (setpoint - process_value) + bias`
- Writes no output for a pair when `cv.<name>.enable == false` or `mv.<name>.enable == false`

### Linear MPC

- Uses the persisted continuous model `x_dot = A x + B u`, `y = C x + D u`
- Discretizes each control cycle with zero-order hold/RK4
- Keeps `u_order` as `MVs + DVs`; MVs are optimized and measured DVs remain constant over the horizon
- Required I/O:
  - Per CV: `cv.<name>.setpoint` and `cv.<name>.process_value`
  - Per MV: `mv.<name>.write`; `mv.<name>.read` is optional and falls back to the last manipulated value
  - Per DV: `dv.<name>.read`
- Online parameters:
  - Horizons: `mpc.prediction_horizon`, `mpc.control_horizon`
  - CV tuning: `mpc.cv.<cv>.q`, `mpc.cv.<cv>.band_low`, `mpc.cv.<cv>.band_high`
  - MV tuning: `mpc.mv.<mv>.r`, `mpc.mv.<mv>.max_delta_u`, `mpc.mv.<mv>.use_l1` (boolean, default `false`)
  - Kalman filter: `mpc.kalman.initial_p`, `mpc.kalman.state.<idx>.q`, `mpc.kalman.cv.<cv>.r`, `mpc.kalman.cv.<cv>.bias_q`

Each MV has a **Movement cost** selector in Control Strategy and Online Parameters. The default
**L2 — squared movement** applies `r * delta_u^2`, strongly penalizing large moves and favoring
gradual adjustments. **L1 — absolute movement** applies `r * abs(delta_u)` and encourages zero
moves. L1 charges by move size; it does not charge a fixed fee per switching event. Both retain
quadratic CV tracking and use the same convex QP solver. The same `r` weight scales either mode,
so changing modes may require retuning it; weights are not automatically rescaled.

`mpc.mv.<mv>.use_l1 = true` selects L1 for that MV; `false` or a missing setting selects L2.
Online changes use **Apply**, persist to the project, and take effect on the next controller
calculation without resetting the Kalman observer. Manual parameter updates remain locked during
online optimization. Numeric parameter optimization leaves the boolean norm choice fixed;
offline actuator scoring and online delta-MV scoring continue to use squared movement regardless
of the controller's selected norm.

CV bands are relative to the live setpoint. Error inside the band is not penalized; error outside
the band is optimized toward the nearest boundary. Runtime and simulation logs expose plot-ready
MPC diagnostics such as `mpc.xhat.*`, `mpc.bias.cv.*`, `mpc.du.*`, `mpc.target.*`,
`mpc.pred.free.*`, `mpc.pred.mpc.*`, `future_pred_*`, and `mpc.status.*`.

### Kalman Filter Tuning

After saving either a compatible Linear MPC strategy or a compatible Custom Rust strategy with a
complete observer parameter set:

1. Import historical measurements from a UTF-8, comma-separated CSV file.
2. Adjust Kalman parameters and optionally persist them to `project.toml`.
3. Run the observer.
4. Open interactive Plotly charts.

The CSV requires a strictly increasing RFC 3339/ISO 8601 `time` column with a time zone, one
column for each configured CV, MV, and DV, and at least two complete finite data rows.

```csv
time,y1,u1,d1
2026-06-13T14:30:00.000+02:00,1.2,0.3,0.0
2026-06-13T14:30:01.000+02:00,1.3,0.4,0.1
```

Additional columns are ignored. Imported data and observer results remain in the current desktop
session. Interactive plots require the local web API.

Linear MPC uses the saved `mpc.kalman.*` parameters. Custom Rust strategies can enable the same
tuning task by exposing numeric online parameters with either `kalman.*` or `observer.*` names:

- `kalman.initial_p` or `observer.initial_p`
- `kalman.state.<idx>.q` or `observer.state.<idx>.q`
- `kalman.cv.<cv>.r` or `observer.cv.<cv>.r`
- `kalman.cv.<cv>.bias_q` or `observer.cv.<cv>.bias_q`

All configured states and CVs must be covered. The replay maps these aliases to the internal
Kalman observer while saving edits back to the original exposed custom parameter names.

## Custom Rust Control Strategy

Prerequisites:

- A local Rust toolchain with `cargo`
- Defined project I/O
- Stopped runtime and simulation for structural configuration changes

Workflow:

1. Open the **Control Strategy** tab and generate a template.
2. Edit `projects/<project>/logic/custom_strategy/src/lib.rs`.
3. Implement `control(...)` and optionally add `f64` or `bool` arguments after `ctx` and `out`.
4. Build and check the strategy.
5. Select and save it after the contract matches the current I/O definition.

Required signature:

```rust
pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs)
```

Signature with online parameters:

```rust
pub fn control(
    ctx: &SmartApcContext,
    out: &mut SmartApcOutputs,
    gain: f64,
    enable_ff: bool,
)
```

The context provides current inputs and input/output/parameter history. Set every required
`mv.<name>.write` output through `out`. Use `out.log(...)`, `out.log_value(...)`, and
`out.log_values(...)` for structured user logs.

Custom strategies can provide dotted future-prediction lines by logging `F64` values with:

- `future_pred_without_control.cv.<cv>.kNN`
- `future_pred_with_control.cv.<cv>.kNN`
- `future_pred_with_control.mv.<mv>.kNN`

`NN` is the one-based, two-digit horizon step, for example `k01`.

Minimal example:

```rust
pub fn control(
    ctx: &SmartApcContext,
    out: &mut SmartApcOutputs,
    kp: f64,
) {
    let setpoint = match ctx.inputs.get("cv.y1.setpoint") {
        Some(SignalValue::F64(v)) => *v,
        _ => 0.0,
    };
    let process_value = match ctx.inputs.get("cv.y1.process_value") {
        Some(SignalValue::F64(v)) => *v,
        _ => 0.0,
    };
    out.set_f64("mv.u1.write", kp * (setpoint - process_value));
}
```

Static I/O values without OPC UA Node IDs are exposed as online parameters:

- `io.mv.<name>.hl`
- `io.mv.<name>.ll`
- `io.cv.<name>.deadband`

## Important Notes

- V1 is local and OT-focused, with no cloud dependency.
- Cycle times are configured independently in milliseconds:
  - `communication.cycle_ms` drives OPC UA health checks and the watchdog.
  - `io.cycle_ms` drives input/output exchanges and must be a multiple of the communication cycle.
  - `logic.cycle_ms` drives controller execution, MPC/Kalman discretization, tuning, and simulation.
- All cycle times must be greater than zero.

## API Quick Reference

- `GET /api/runtime/state`
- `GET /api/runtime/params`
- `PUT /api/runtime/params`
- `POST /api/simulation/start`
- `POST /api/simulation/stop`
- `GET /api/simulation/state`
- `GET /api/execution/state`
- `GET /api/execution/params`
- `GET /api/plots?limit=250`
- `GET /api/runtime/plots/window?window_ms=180000`
- `GET /api/runtime/plots/catalog`
- `GET /api/execution/plots/window?window_ms=180000`
- `GET /api/execution/plots/catalog`
- `GET /api/projects/{name}/plots/config`
- `PUT /api/projects/{name}/plots/config`
- `POST /api/simulate/step`
