# AGENTS.md Workflow Playbook (AI + Human, Pragmatic with Gates)

## 1. Purpose
- Audience: AI agents and human contributors.
- Scope: contributor workflow, quality gates, and runtime validation habits for SmartAPC.
- Goal: keep changes safe, testable, readable, and easy to onboard.

## 2. Project Snapshot
- Repository map:
  - `src/` -> runtime, desktop GUI, web API, protocol adapters, logic, virtual hardware.
  - `projects/` -> per-project configs, runtime DB/logs, custom strategy artifacts.
  - `README.md` -> current product status, run commands, custom strategy guide.
- Primary run modes:
  - `cargo run --features legacy-native -- desktop` (optional native compatibility UI)
  - `cargo run -- serve`
  - `cargo run -- virtual-hardware`
- Additional helper mode:
  - `cargo run -- init-project --name <name> --project-root ./projects`

## 3. Golden Workflow
1. Understand the task and expected user-visible behavior.
2. Inspect only relevant modules/files first.
3. Implement a focused change (smallest safe scope).
4. Run required gates.
5. Summarize impact, risks, and follow-up actions.
- Rule: avoid unrelated edits; preserve existing behavior unless explicitly requested.
- Browser I/O CSV import replaces MV/CV/DV/custom definitions only after complete
  Rust validation; persistence uses the existing configuration save gates. Custom
  variables are typed f64/bool, require read/write Node IDs (one or both), and use
  `custom.<name>.read/write` in strategy contracts without changing model dimensions.
  Generated strategy output defaults must preserve each configured signal type.
- Browser plant-model import uses one labeled A/B/C/D CSV and a template generated
  from draft MV/CV/DV names and state dimensions. Validate the complete file in Rust
  before replacing the draft; persistence uses the existing configuration save gates.

## 4. Must Gates Before Commit
- Default required gates (unless docs-only task):
  - `cargo fmt`
  - `cargo check`
  - `cargo test`
- If a gate cannot run, document why and what remains unverified.
- For browser application changes also run `npm run typecheck`, `npm run build`,
  and `npm test` in `frontend/`. Use `npm run test:e2e` for meaningful browser
  workflows against the isolated Rust backend, with Node.js 22 or newer.
  Track full migration evidence in `docs/web-migration-coverage.md`; keep native
  helpers behind `legacy-native`; also run `cargo check --features legacy-native`
  and `cargo test --features legacy-native` when changing shared/native helpers.
  Package with `python3 scripts/package-web.py` and verify adjacent `ui/` assets
  from a launch directory outside the repository when changing serving or packaging.

## 5. Code Readability & Onboarding (Must)
- Generated code must be commented and easy to read for fast onboarding.
- Prefer clear names and small composable functions over dense logic.
- Add high-signal comments for non-obvious behavior, contracts, and data flow.
- Generated templates/scaffolds must include usage-oriented comments, not just placeholders.
- Always commit task-related changes before finishing, without asking for confirmation.
  Run the required gates first and keep unrelated user changes out of the commit.

## 6. Commit Conventions
- Prefer multiple clean commits grouped by subsystem/behavior.
- Commit message style: short imperative subject, optional context in body.
- Do not amend/rewrite history unless explicitly requested.

## 7. Safety & Change Boundaries
- No destructive git/file operations unless explicitly requested.
- Do not revert unrelated user changes.
- Runtime/GUI behavior changes must include notes on user-visible impact.

## 8. SmartAPC-Specific Rules
- Keep strategy compatibility logic strict: selected strategy must match current I/O contract.
- Keep online parameters limited to:
  - active logic parameters
  - active static I/O parameters (values without an OPC UA Node ID that are runtime-editable)
- Linear MPC defaults to squared (L2) MV movement cost. Per-MV `mpc.mv.<name>.use_l1`
  is a boolean manual/online selection for absolute (L1) movement cost and must stay out of
  numeric parameter optimization. Offline/online tuning scores retain squared MV movement.
- Ensure logging additions are structured and testable (runtime log entries must be machine-parseable).
- Online optimization events must remain structured. Manual/safety/execution stops restore the
  original parameters before the next controller calculation; only normal convergence/completion
  restores the best-known parameters.
- Runtime starts in shadow mode; actuator writes require explicit manual enable
  or the optional read-only `deployment.external_controller_enable` Boolean node.
  Its presence locks manual enable in the UI and backend. Never persist enable.
- Deployment Boolean outputs `ControllerIsReady` and `ControllerHasControl` use
  dedicated Node IDs. Readiness requires observer convergence and a valid successful
  suggestion; ownership additionally requires enabled, successful actuator writes.
  Faults, missed suggestion deadlines, and stop clear both; watchdog/status writes
  remain active when actuator writes are suppressed. Deployment tests write false
  to status nodes and verify readback; they never write the external enable node.
- Linear MPC observer convergence uses five finite posterior covariance updates
  with relative change <= 0.001. Custom observer strategies report convergence via
  the optional `observer_converged` response; generated outputs document this field.
  Declared observer/Kalman parameters require explicit convergence. Preserve the
  old response contract for custom strategies without observers.
- Keep timing ownership explicit: communication/watchdog use `communication.cycle_ms`, I/O exchanges use `io.cycle_ms`, and controller/runtime simulation/tuning use `logic.cycle_ms`.
- Keep `io.cycle_ms` aligned as a multiple of `communication.cycle_ms`; controller timing remains independent.

## 9. Custom Rust Strategy Workflow (Quick Reference)
1. Generate template in Control Strategy tab.
2. Edit `control(...)` in `projects/<project>/logic/custom_strategy/src/lib.rs`.
3. Build/check strategy via UI button.
4. Verify compatibility result against current I/O definition.
- Available context includes current `inputs`, input/output/parameter history, and online parameters.
- Use user-log helpers from the template to emit custom runtime logs.
- Browser source saves check both configuration and source revisions. Regeneration
  backs up replaced source/manifest/dispatch and saved TOML under `logic/backups/<id>`.
- Scoped browser execution mutations require project, mode and execution ID; online
  parameter writes also require the expected applied values to reject stale clients.
- Browser history is bounded with extrema-preserving sampling. Full job reports
  persist by ID; list/status payloads omit replay arrays and optimization curves.
- Simulation JSONL buffers flush after 64 records or one wall-clock second at the
  next cycle, keeping live user logs visible without changing controller timing.

## 10. Optional Manual Smoke Test (Recommended)
- Start virtual hardware:
  - `cargo run -- virtual-hardware --host 127.0.0.1 --port 4855 --dt-ms 100 --tau-s 5.0 --gain 1.0 --history 120`
- Build frontend assets, then start the browser backend:
  - `cargo run -- serve --project-root ./projects`
- Open `http://127.0.0.1:8080` (plant feedback is on port 8081). In the workspace:
  - Configure OPC UA endpoint and watchdog.
  - Run I/O read/write test.
  - Start runtime and verify plot/log updates.
- This smoke test is recommended, not mandatory for every change.

## 11. Definition of Done
- Required gates pass (or docs-only exception is explicit).
- Behavior is validated to the level required by change risk.
- Docs are updated if workflow/API/UX changed.
- Generated code/comments are reviewed for onboarding readability.

## 12. Keep This File Updated
- Update `AGENTS.md` in the same PR when any of these change:
  - workflow
  - quality gates
  - strategy contract/compatibility logic
  - code-generation style
  - runtime logging behavior

## 13. Web App Migration Skills
- Reusable project skill sources:
  - `skills/smartapc-web-ui/SKILL.md` -> React UI, navigation, and Plotly integration.
  - `skills/smartapc-web-migration/SKILL.md` -> Rust service/API extraction and feature parity.
- The full migration goal is in `docs/prompts/smartapc-web-app-goal.md`.
- For that migration, use both skills and their relevant references. The target is a
  locally served React web app with Rust/Axum as the only backend and Plotly retained.
- Reorganizing setup, engineering, operation, and deployment does not change runtime
  contracts or turn optional tuning/simulation into universal deployment prerequisites.
- Update actual run/build instructions and quality gates alongside implementation;
  the skill and goal files describe the target, not completed application behavior.
