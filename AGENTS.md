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
  - `cargo run -- desktop`
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

## 4. Must Gates Before Commit
- Default required gates (unless docs-only task):
  - `cargo fmt`
  - `cargo check`
  - `cargo test`
- If a gate cannot run, document why and what remains unverified.

## 5. Code Readability & Onboarding (Must)
- Generated code must be commented and easy to read for fast onboarding.
- Prefer clear names and small composable functions over dense logic.
- Add high-signal comments for non-obvious behavior, contracts, and data flow.
- Generated templates/scaffolds must include usage-oriented comments, not just placeholders.

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
  - active static I/O parameters (values without OPC address that are runtime-editable)
- Ensure logging additions are structured and testable (runtime log entries must be machine-parseable).

## 9. Custom Rust Strategy Workflow (Quick Reference)
1. Generate template in Regellogik tab.
2. Edit `control(...)` in `projects/<project>/logic/custom_strategy/src/lib.rs`.
3. Build/check strategy via UI button.
4. Verify compatibility result against current I/O definition.
- Available context includes current `inputs`, input/output/parameter history, and online parameters.
- Use user-log helpers from the template to emit custom runtime logs.

## 10. Optional Manual Smoke Test (Recommended)
- Start virtual hardware:
  - `cargo run -- virtual-hardware --host 127.0.0.1 --port 4855 --dt-ms 100 --tau-s 5.0 --gain 1.0 --history 120`
- Start desktop app:
  - `cargo run -- desktop --project-root ./projects`
- In GUI:
  - Configure OPC-UA endpoint and watchdog.
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
