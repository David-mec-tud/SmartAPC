# SmartAPC

SmartAPC V1 is modular control software written in Rust for OT and industrial environments.

## Browser workspace (October 2026)

SmartAPC runs as a local React/TypeScript application served by Rust/Axum. The
workspace includes project configuration, typed MV/CV/DV I/O, model matrices,
P/MPC/custom Rust strategies, simulation, control tests, Kalman replay, offline and
online optimization, live parameters, structured logs, diagnostics and deployment.
Plotly charts, icons, scripts and styles are packaged locally, with light/dark
styles and no required external account or CDN.

Rust owns the controller, OPC UA, watchdog, independent cycle times and background
jobs. Closing a browser does not stop execution or engineering jobs. Projects
remain TOML, with SQLite history and JSONL logs. Fault transitions remain
`Running -> Hold -> Alarm -> Off`; faults suppress actuator writes and clear the
controller handshake flags. Online tuning restores
original values on manual/safety/execution stops and applies best values on normal
completion. The optional continuous model uses `x_dot = A x + B u`, `y = C x + D u`;
offline simulation integrates it with RK4. Runtime does not simulate the plant.

Deployment validates saved strategy requirements, generates Copy/Download commands,
and offers explicit runtime start and verification in shadow mode. Simulation and tuning remain
optional. Windows Service Control Manager integration is **not implemented or
verified**; Windows service commands are reference generation only.

## Import a plant model

In **Configuration → Plant model**, download one CSV template for all four model
matrices (A/B/C/D), generated from the current MV/CV/DV draft and state dimensions.
Replace the `a11`, `a12`, `b11`, etc. placeholders with numeric coefficients and
upload the completed file with **Import model CSV**. Keep the matrix headings and
signal labels in order. The entire import is validated before updating the draft;
use **Save configuration** to persist it. Manual matrix editing remains available.
See the [CSV layout and example](docs/web-application.md#model-csv-import).

## Controller deployment handshake

In **Deployment → Controller OPC UA handshake**, save optional Boolean Node IDs:

- `ControllerIsReady`: SmartAPC writes true after the observer converges and a
  valid control suggestion succeeds. P control has no observer warm-up.
- `ControllerHasControl`: SmartAPC writes true only while enabled, ready, and
  successfully writing actuator outputs.
- `ExternalControllerEnable`: optional read-only input; when configured, it owns
  controller on/off and disables the UI's manual button. SmartAPC never writes it.

**Test saved controller signals** checks Boolean types, writes false to the two
status outputs, and confirms readback. It only reads the external enable. Node IDs
must be distinct from each other, I/O, and watchdog nodes. Blank status addresses
support existing local-only projects; their state remains visible in the UI.

Every runtime start, including `serve --project`, resets to shadow mode. The
observer and suggestions run while actuator writes wait for **Turn controller on**
or a true external enable. Turning off returns to shadow mode without resetting
the observer. Status output writes and watchdog exchanges continue in shadow mode.
Faults, invalid/empty suggestions, MPC solver fallback, missed suggestion deadlines,
and stop clear readiness/control. Healthy fresh cycles can establish readiness
again. A failed external read also clears the requested enable.

Linear MPC convergence requires five consecutive finite posterior covariance
updates with relative matrix change at most `0.001`. Custom strategies can return
`observer_converged`; generated `SmartApcOutputs` exposes `Option<bool>` for this.
Strategies declaring observer/Kalman parameters must report `Some(true)` explicitly;
older strategies without those parameters remain compatible. Custom observers
without such parameters should report `Some(false)` during warm-up.

The deadline monitor runs independently of synchronous calculations. Each scheduled
suggestion has one `logic.cycle_ms` window to complete; missed calculation windows
or skipped cycles revoke readiness and late results cannot reassert it. Flag writes
are attempted immediately on faults and at exchanges. If OPC UA is disconnected,
the device cannot receive false until transport is available; its watchdog remains
necessary to detect loss of the software.

## Build, Test, and Run

Use Rust and Node.js 22 or newer to build. The standard Rust build does not depend
on iced, eframe/egui or native file dialogs.

```sh
cd frontend
npm ci
npm run build
cd ..
cargo run -- serve --project-root ./projects
```

Open `http://127.0.0.1:8080`. For development, run `npm run dev` in `frontend/`
and open `http://127.0.0.1:5173`; Vite proxies the API to Axum on port 8080.

Build a relocatable release package (Python 3 is used only by the build script):

```sh
python3 scripts/package-web.py --output ./dist/smartapc-web
./dist/smartapc-web/smartapc serve --project-root ./projects
```

Keep the executable and its adjacent `ui/` folder together. Launch from any
working directory with an absolute `--project-root`; deployed operation needs
no Node or Python process. Custom Rust strategy compilation still needs Cargo.
The script builds for the current host and writes a SHA-256 package manifest;
it does not cross-compile or install services. See [build/run/API details](docs/web-application.md)
and [migration evidence](docs/web-migration-coverage.md).

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

The previous native application remains available with an explicit compatibility
feature. Native builds require their platform GUI dependencies:

```sh
cargo run --features legacy-native -- desktop --project-root ./projects
cargo run --features legacy-native -- desktop --project-root ./projects --no-web
cargo check --features legacy-native
cargo test --features legacy-native
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

2. Start the browser backend in a second terminal:

```bash
cargo run -- serve --project-root ./projects
```

3. Open `http://127.0.0.1:8080` and create/select a project. In the application:
   - Set the endpoint to `opc.tcp://127.0.0.1:4855/`.
   - Configure `read_node = ns=2;s=wd_counter_read` and `write_node = ns=2;s=wd_counter_mirror`.
   - Define variables and I/O, for example `cv.y1.*` and `mv.u1.*`.
   - Select a control strategy, configure/test deployment signals, start in shadow
     mode, and explicitly turn the controller on.

The plant feedback page is `http://127.0.0.1:8081/virtual-hardware`, with local
Plotly charts and manual node values. Use `--no-web` for OPC UA only, or
`--features legacy-native -- virtual-hardware --native-plot` for the old plot window.

Virtual hardware OPC UA Node IDs:

- `ns=2;s=y1_value`
- `ns=2;s=y1_target`
- `ns=2;s=y1_HL`
- `ns=2;s=y1_LL`
- `ns=2;s=u1_write`
- `ns=2;s=u1_read`
- `ns=2;s=wd_counter_read`
- `ns=2;s=wd_counter_mirror`
- `ns=2;s=ControllerIsReady` (Boolean)
- `ns=2;s=ControllerHasControl` (Boolean)
- `ns=2;s=ExternalControllerEnable` (Boolean)

## Control Strategies

The **Project configuration → Control strategy** page offers only strategies compatible with the current I/O definition:

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

Additional columns are ignored. Browser replay reports, including imported measurements and
observer series, persist under `data/jobs/` and can be reopened or downloaded after restart.
The optional native helper retains its session-based replay behavior.

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

1. Open **Project configuration → Control strategy** and generate a template.
2. Edit the source in the browser or `projects/<project>/logic/custom_strategy/src/lib.rs`.
   Save browser source edits before building. Regeneration backs up replaced source, manifest,
   dispatch and saved project under `logic/backups/<id>/`.
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

## Legacy API Quick Reference

The browser uses revision- and execution-scoped APIs documented in
[web application contracts](docs/web-application.md). Existing helper routes remain available.

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
