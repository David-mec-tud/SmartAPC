import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router-dom";
import type { Data } from "plotly.js";
import { type Mode, type Parameters, projectPath, request } from "../api";
import { useWorkspace } from "../workspace";
import {
  Button,
  Badge,
  Field,
  Input,
  Notice,
  NumberInput,
  Select,
} from "../components/ui";
import { Plot } from "../components/Plot";
import { TrendDashboard as Trends } from "../components/TrendDashboard";
export { Trends };
import { groupParameters, parameterLabel } from "../parameters";
import { RangeTable } from "./Engineering";

export function Operation() {
  const { section = "trends" } = useParams();
  const { project, execution } = useWorkspace();
  return (
    <>
      <div className="page-title">
        <div className="eyebrow">Operation</div>
        <h1>
          {{
            trends: "Live & historical trends",
            parameters: "Online parameters",
            optimization: "Online optimization",
            logs: "Logs & events",
            diagnostics: "Execution diagnostics",
          }[section] || section}
        </h1>
        <p>
          Operational actions apply to the project and execution identified by
          Rust.
        </p>
      </div>
      {section === "trends" ? (
        <Trends />
      ) : section === "parameters" ? (
        <OnlineParameters
          key={`${project}:${execution?.snapshot.execution_id}`}
        />
      ) : section === "optimization" ? (
        <OnlineOptimization />
      ) : section === "logs" ? (
        <Logs />
      ) : (
        <Diagnostics />
      )}
    </>
  );
}

function OnlineParameters() {
  const { project, execution, workspace, act } = useWorkspace();
  const active = execution?.snapshot.project_name === project;
  const params = useQuery({
    queryKey: ["online-params", project, execution?.snapshot.execution_id],
    queryFn: async () =>
      (
        await request<{ params: Parameters }>(
          `/api/execution/parameters?project_name=${encodeURIComponent(project)}&mode=${execution?.mode}&execution_id=${execution?.snapshot.execution_id}`,
        )
      ).params,
    enabled: active,
    refetchInterval: 1500,
  });
  const [draft, setDraft] = useState<Parameters>({});
  const [expected, setExpected] = useState<Parameters>({});
  const [applying, setApplying] = useState<Record<string, boolean>>({});
  const [search, setSearch] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  useEffect(() => {
    setDraft({});
    setExpected({});
  }, [project, execution?.snapshot.execution_id]);
  const locked = workspace.data?.online_optimization?.phase === "running";
  const groups = groupParameters(params.data ?? {}, search);
  const matchCount = groups.reduce((count, [, rows]) => count + rows.length, 0);
  if (!active)
    return (
      <Notice>
        Select the active execution project to edit its online parameters.
      </Notice>
    );
  return (
    <section className="panel">
      <h2>Active logic and static I/O parameters</h2>
      <p className="muted">
        Applied values are read from Rust. Apply writes and persists a typed
        value for this {execution.mode}.
      </p>
      {locked && (
        <Notice>Manual edits are locked while online optimization runs.</Notice>
      )}
      <Input
        aria-label="Search parameters"
        placeholder="Search parameters…"
        value={search}
        onChange={(event) => setSearch(event.target.value)}
      />
      {params.error && <Notice error>{params.error.message}</Notice>}
      <p className="muted">
        {matchCount} of {Object.keys(params.data ?? {}).length} parameters
      </p>
      {!matchCount && (
        <Notice>
          {search
            ? "No parameters match your search."
            : "No editable parameters are available."}
        </Notice>
      )}
      {groups.map(([group, entries]) => (
        <details
          className="parameter-group"
          key={group}
          open={
            search.trim()
              ? true
              : (expanded[group] ?? group === "Controller settings")
          }
          onToggle={(event) => {
            if (!search.trim()) {
              const open = event.currentTarget.open;
              setExpanded((previous) =>
                previous[group] === open
                  ? previous
                  : { ...previous, [group]: open },
              );
            }
          }}
        >
          <summary>
            {group} · {entries.length}
          </summary>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Parameter</th>
                  <th>Applied</th>
                  <th>Draft value</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {entries.map(([key, value]) => (
                  <tr key={key}>
                    <th>
                      {parameterLabel(key)}
                      <small className="parameter-key">{key}</small>
                    </th>
                    <td>{String(value)}</td>
                    <td>
                      {typeof value === "boolean" ? (
                        <Select
                          aria-label={key}
                          disabled={locked}
                          value={String(draft[key] ?? value)}
                          onChange={(event) => {
                            setExpected((previous) => ({
                              ...previous,
                              [key]:
                                draft[key] === undefined
                                  ? value
                                  : (previous[key] ?? value),
                            }));
                            setDraft((previous) => ({
                              ...previous,
                              [key]: event.target.value === "true",
                            }));
                          }}
                        >
                          <option value="false">
                            {key.endsWith(".use_l1")
                              ? "L2 · squared movement"
                              : "False"}
                          </option>
                          <option value="true">
                            {key.endsWith(".use_l1")
                              ? "L1 · absolute movement"
                              : "True"}
                          </option>
                        </Select>
                      ) : (
                        <NumberInput
                          aria-label={key}
                          disabled={locked}
                          value={Number(draft[key] ?? value)}
                          onChange={(next) => {
                            setExpected((previous) => ({
                              ...previous,
                              [key]:
                                draft[key] === undefined
                                  ? value
                                  : (previous[key] ?? value),
                            }));
                            setDraft((previous) => ({
                              ...previous,
                              [key]: next ?? 0,
                            }));
                          }}
                        />
                      )}
                    </td>
                    <td>
                      <Button
                        size="small"
                        disabled={
                          locked || applying[key] || draft[key] === undefined
                        }
                        onClick={() => {
                          const submitted = draft[key];
                          setApplying((previous) => ({
                            ...previous,
                            [key]: true,
                          }));
                          void act("/api/execution/parameters", {
                            project_name: project,
                            mode: execution.mode,
                            execution_id: execution.snapshot.execution_id,
                            params: { [key]: draft[key] },
                            expected_params: { [key]: expected[key] },
                          })
                            .then(() => {
                              setDraft((previous) => {
                                const next = { ...previous };
                                if (next[key] === submitted) delete next[key];
                                return next;
                              });
                              setExpected((previous) => ({
                                ...previous,
                                [key]: submitted,
                              }));
                              void params.refetch();
                            })
                            .catch(() => {})
                            .finally(() =>
                              setApplying((previous) => ({
                                ...previous,
                                [key]: false,
                              })),
                            );
                        }}
                      >
                        Apply online
                      </Button>
                      {draft[key] !== undefined && expected[key] !== value && (
                        <small>
                          Applied value changed. Keep or reload your draft.
                        </small>
                      )}
                      {draft[key] !== undefined && (
                        <Button
                          variant="ghost"
                          size="small"
                          aria-label={`Reload ${key} applied value`}
                          onClick={() => {
                            setDraft((previous) => {
                              const next = { ...previous };
                              delete next[key];
                              return next;
                            });
                            setExpected((previous) => {
                              const next = { ...previous };
                              delete next[key];
                              return next;
                            });
                          }}
                        >
                          Reload applied value
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </details>
      ))}
    </section>
  );
}
function OnlineOptimization() {
  const {
    config: cfg,
    edit,
    project,
    execution,
    workspace,
    act,
    dirty,
    saved,
    acceptSavedSection,
    catalog,
  } = useWorkspace();
  if (!cfg) return null;
  const opt = cfg.online_optimization;
  const status = workspace.data?.online_optimization;
  const active = execution?.snapshot.project_name === project;
  const running = active && status?.phase === "running";
  return (
    <>
      <Notice>
        Touching a CV safety edge or stopping manually restores the original
        parameters before the next calculation. Normal completion applies
        best-known values. optimization_mode_LL/HL are absolute safety
        boundaries.
      </Notice>
      <section className="panel">
        <div className="section-title">
          <h2>Optimization session</h2>
          <Badge tone={running ? "accent" : "neutral"}>
            {active
              ? (status?.phase ?? "Ready")
              : "Select active execution project"}
          </Badge>
        </div>
        <div className="inline">
          <Button
            disabled={!active || running || dirty}
            onClick={() =>
              void act("/api/execution/optimization", {
                action: "start",
                project_name: project,
                mode: execution?.mode,
                execution_id: execution?.snapshot.execution_id,
              }).catch(() => {})
            }
          >
            Start saved optimization
          </Button>
          <Button
            variant="destructive"
            disabled={!running}
            onClick={() =>
              void act("/api/execution/optimization", {
                action: "stop",
                project_name: project,
                mode: execution?.mode,
                execution_id: execution?.snapshot.execution_id,
              }).catch(() => {})
            }
          >
            Stop & restore originals
          </Button>
          <Button
            variant="outline"
            disabled={!active || status?.phase !== "finished"}
            onClick={() =>
              void act("/api/execution/optimization", {
                action: "save_best",
                project_name: project,
                mode: execution?.mode,
                execution_id: execution?.snapshot.execution_id,
              }).catch(() => {})
            }
          >
            Save best-known parameters
          </Button>
        </div>
        {active && status && (
          <>
            <div className="metrics-grid">
              <div>
                <small>Iteration</small>
                <strong>
                  {status.iteration} / {status.max_iterations}
                </strong>
              </div>
              <div>
                <small>Window samples</small>
                <strong>
                  {status.samples_in_window} / {status.samples_per_window}
                </strong>
              </div>
              <div>
                <small>Best objective</small>
                <strong>{status.best_score?.toPrecision(5) ?? "—"}</strong>
              </div>
            </div>
            {(status.safety_reason || status.finish_reason) && (
              <Notice error={!!status.safety_reason}>
                {status.safety_reason || status.finish_reason}
              </Notice>
            )}
            <pre>
              {JSON.stringify(
                { current: status.current_params, best: status.best_params },
                null,
                2,
              )}
            </pre>
          </>
        )}
      </section>
      <fieldset disabled={running} className="editor-fieldset">
        <section className="panel">
          <div className="section-title">
            <h2>Window and search</h2>
            <Button
              variant="outline"
              disabled={running || !saved.data}
              onClick={() =>
                void act(
                  `${projectPath(project)}/online-optimization/config`,
                  { revision: saved.data?.revision, config: opt },
                  "PUT",
                )
                  .then(() => acceptSavedSection("online_optimization"))
                  .catch(() => {})
              }
            >
              Save optimization objectives
            </Button>
          </div>
          <div className="form-grid">
            {(
              [
                "duration_s",
                "max_iterations",
                "initial_samples",
                "convergence_patience",
              ] as const
            ).map((key) => (
              <Field key={key} label={key.replaceAll("_", " ")}>
                <NumberInput
                  value={opt[key]}
                  onChange={(value) =>
                    edit((next) => {
                      next.online_optimization[key] = value ?? 0;
                    })
                  }
                />
              </Field>
            ))}
          </div>
          <h3>Choose numeric parameters</h3>
          <div className="series-picker">
            {Object.entries(
              catalog.data?.online_optimization_parameters ?? {},
            ).map(([key, value]) => (
              <label className="check-label" key={key}>
                <input
                  type="checkbox"
                  checked={opt.parameters.some((row) => row.key === key)}
                  onChange={(event) =>
                    edit((next) => {
                      next.online_optimization.parameters =
                        next.online_optimization.parameters.filter(
                          (row) => row.key !== key,
                        );
                      if (event.target.checked)
                        next.online_optimization.parameters.push({
                          key,
                          min: value >= 0 ? 0 : value * 2,
                          max: value > 0 ? value * 2 : 1,
                        });
                    })
                  }
                />
                {key}
              </label>
            ))}
          </div>
          <RangeTable
            online
            parameters={opt.parameters}
            onChange={(index, key, value) =>
              edit((next) => {
                (
                  next.online_optimization.parameters[
                    index
                  ] as unknown as Record<string, unknown>
                )[key] = value;
              })
            }
          />
        </section>
        <section className="panel">
          <h2>CV objectives and safety boundaries</h2>
          <div className="inline">
            {Object.entries(catalog.data?.cv_safety_limits ?? {}).map(
              ([name, limit]) => (
                <label className="check-label" key={name}>
                  <input
                    type="checkbox"
                    checked={opt.cvs.some((row) => row.name === name)}
                    onChange={(event) =>
                      edit((next) => {
                        next.online_optimization.cvs =
                          next.online_optimization.cvs.filter(
                            (row) => row.name !== name,
                          );
                        if (event.target.checked)
                          next.online_optimization.cvs.push({
                            name,
                            weight: 1,
                            low: limit.min,
                            high: limit.max,
                          });
                      })
                    }
                  />
                  {name}
                </label>
              ),
            )}
          </div>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>CV</th>
                  <th>Weight</th>
                  <th>optimization_mode_LL</th>
                  <th>optimization_mode_HL</th>
                </tr>
              </thead>
              <tbody>
                {opt.cvs.map((row, index) => (
                  <tr key={row.name}>
                    <th>{row.name}</th>
                    {(["weight", "low", "high"] as const).map((key) => (
                      <td key={key}>
                        <NumberInput
                          aria-label={`${row.name} ${key}`}
                          value={row[key]}
                          onChange={(value) =>
                            edit((next) => {
                              next.online_optimization.cvs[index][key] =
                                value ?? 0;
                            })
                          }
                        />
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
        <section className="panel">
          <h2>Squared MV movement objectives</h2>
          <div className="inline">
            {cfg.variables.mvs.map((name) => (
              <label className="check-label" key={name}>
                <input
                  type="checkbox"
                  checked={opt.delta_mvs.some((row) => row.name === name)}
                  onChange={(event) =>
                    edit((next) => {
                      next.online_optimization.delta_mvs =
                        next.online_optimization.delta_mvs.filter(
                          (row) => row.name !== name,
                        );
                      if (event.target.checked)
                        next.online_optimization.delta_mvs.push({
                          name,
                          weight: 1,
                        });
                    })
                  }
                />
                {name}
              </label>
            ))}
          </div>
          <div className="form-grid">
            {opt.delta_mvs.map((row, index) => (
              <Field key={row.name} label={`${row.name} weight`}>
                <NumberInput
                  value={row.weight}
                  onChange={(value) =>
                    edit((next) => {
                      next.online_optimization.delta_mvs[index].weight =
                        value ?? 0;
                    })
                  }
                />
              </Field>
            ))}
          </div>
        </section>
      </fieldset>
    </>
  );
}
function Logs() {
  const { project, execution } = useWorkspace();
  const [mode, setMode] = useState<Mode>(execution?.mode ?? "runtime");
  const [search, setSearch] = useState("");
  const logs = useQuery({
    queryKey: ["logs", project, mode],
    queryFn: () =>
      request<Record<string, unknown>[]>(
        `${projectPath(project)}/logs?mode=${mode}`,
      ),
    enabled: !!project,
    refetchInterval: 2000,
  });
  return (
    <>
      <div className="toolbar">
        <Field label="Log source">
          <Select
            value={mode}
            onChange={(event) => setMode(event.target.value as Mode)}
          >
            <option value="runtime">Runtime JSONL</option>
            <option value="simulation">Simulation JSONL</option>
          </Select>
        </Field>
        <Input
          aria-label="Search logs"
          placeholder="Filter events, user logs, errors…"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
      </div>
      {logs.error && <Notice error>{logs.error.message}</Notice>}
      <section className="panel log-list">
        {logs.data
          ?.filter((row) =>
            JSON.stringify(row).toLowerCase().includes(search.toLowerCase()),
          )
          .map((row, index) => (
            <details key={index}>
              <summary>
                <code>
                  {String(
                    row.timestamp ??
                      row.timestamp_ms ??
                      row.simulation_time_s ??
                      "",
                  )}
                </code>
                <strong>
                  {String(row.event ?? row.kind ?? row.type ?? "cycle")}
                </strong>
                <span>{String(row.state ?? "")}</span>
              </summary>
              <pre>{JSON.stringify(row, null, 2)}</pre>
            </details>
          ))}
        {!logs.data?.length && (
          <p className="empty">No structured log entries yet.</p>
        )}
      </section>
    </>
  );
}
export function Diagnostics() {
  const { execution, workspace } = useWorkspace();
  if (!execution)
    return (
      <Notice>
        The backend is idle. Start a saved project or simulation to inspect its
        execution.
      </Notice>
    );
  return (
    <section className="panel">
      <h2>
        {execution.snapshot.project_name} · {execution.mode}
      </h2>
      <div className="metrics-grid">
        {Object.entries(execution.snapshot.metrics).map(([key, value]) => (
          <div key={key}>
            <small>{key.replaceAll("_", " ")}</small>
            <strong>{value.toLocaleString()}</strong>
          </div>
        ))}
      </div>
      <p>
        State: {execution.snapshot.state} · Watchdog:{" "}
        {JSON.stringify(execution.snapshot.watchdog_status ?? "not applicable")}
      </p>
      <div className="editor-grid">
        <div>
          <h3>Last controller inputs</h3>
          <pre>{JSON.stringify(execution.snapshot.last_inputs, null, 2)}</pre>
        </div>
        <div>
          <h3>Last controller outputs</h3>
          <pre>{JSON.stringify(execution.snapshot.last_outputs, null, 2)}</pre>
        </div>
      </div>
    </section>
  );
}
