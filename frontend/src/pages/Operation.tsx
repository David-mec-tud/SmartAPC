import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router-dom";
import type { Data } from "plotly.js";
import {
  type Mode,
  type Parameters,
  type PlotConfig,
  type Sample,
  type Series,
  projectPath,
  request,
} from "../api";
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
import { RangeTable } from "./Engineering";

export function Operation() {
  const { section = "trends" } = useParams();
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
        <OnlineParameters />
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

export function makeTraces(
  samples: Sample[],
  selected: Series[],
  mode: Mode,
): Data[] {
  return selected.map((series) => {
    const points = samples.filter(
      (row) =>
        row.direction === series.direction && row.signal === series.signal,
    );
    // Bound display work independently of backend/control frequency.
    const step = Math.max(1, Math.ceil(points.length / 1800));
    const displayed = points.filter(
      (_, index) => index % step === 0 || index === points.length - 1,
    );
    return {
      x: displayed.map((row) =>
        mode === "simulation"
          ? row.timestamp_ms / 1000
          : new Date(row.timestamp_ms),
      ),
      y: displayed.map((row) => Number(row.value.value)),
      name: `${series.signal} · ${series.direction}`,
      uid: `${series.direction}::${series.signal}`,
      type: "scatter",
      mode: "lines",
      line: {
        shape: series.value_kind === "bool" ? "hv" : "linear",
        color: signalColor(`${series.direction}::${series.signal}`),
      },
    };
  });
}
function signalColor(identity: string) {
  let hash = 0;
  for (const character of identity)
    hash = (hash * 31 + character.charCodeAt(0)) | 0;
  const palette = [
    "#2c8a7a",
    "#527ad4",
    "#b17a34",
    "#b86d9b",
    "#6d8a46",
    "#aa6458",
  ];
  return palette[Math.abs(hash) % palette.length];
}
function futureTraces(
  samples: Sample[],
  kind: "mv" | "cv",
  name: string,
  mode: Mode,
  cycleMs: number,
  futureSec: number,
): Data[] {
  if (!futureSec || !samples.length) return [];
  const prefixes =
    kind === "cv"
      ? ["future_pred_without_control.cv", "future_pred_with_control.cv"]
      : ["future_pred_with_control.mv"];
  return prefixes.map((prefix) => {
    const matches = samples.filter(
      (row) =>
        row.signal.startsWith(`${prefix}.${name}.k`) &&
        row.value.kind === "f64",
    );
    const stamp = matches.reduce(
      (latest, row) => Math.max(latest, row.timestamp_ms),
      0,
    );
    const horizon = matches
      .filter((row) => row.timestamp_ms === stamp)
      .map((row) => ({
        step: Number(row.signal.split(".k").at(-1)),
        value: Number(row.value.value),
      }))
      .filter((row) => row.step * cycleMs <= futureSec * 1000)
      .sort((a, b) => a.step - b.step);
    return {
      x: horizon.map((row) =>
        mode === "simulation"
          ? (stamp + row.step * cycleMs) / 1000
          : new Date(stamp + row.step * cycleMs),
      ),
      y: horizon.map((row) => row.value),
      type: "scatter",
      mode: "lines",
      name: prefix.includes("without")
        ? "Future · no control"
        : "Future · with control",
      line: { dash: "dot" },
    };
  });
}
export function Trends({ compact = false }: { compact?: boolean }) {
  const { project, saved, execution, act, setNotification } = useWorkspace();
  const [mode, setMode] = useState<Mode>("runtime");
  const [group, setGroup] = useState<"mv" | "cv" | "dv" | "custom">("cv");
  const [plotConfig, setPlotConfig] = useState<PlotConfig>();
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [customTitle, setCustomTitle] = useState("");
  const [customSeries, setCustomSeries] = useState<string[]>([]);
  const active = execution?.snapshot.project_name === project;
  useEffect(() => {
    if (active && execution) setMode(execution.mode);
  }, [active, execution?.mode]);
  useEffect(() => {
    setPlotConfig(saved.data?.config.plots);
    setSelected({});
    setCustomSeries([]);
  }, [project, saved.data?.config.plots]);
  const history = useQuery({
    queryKey: ["history", project, mode, plotConfig?.window_sec],
    queryFn: () =>
      request<Sample[]>(
        `${projectPath(project)}/history?mode=${mode}&window_ms=${(plotConfig?.window_sec ?? 180) * 1000}`,
      ),
    enabled: !!project,
    refetchInterval: active ? 1500 : false,
  });
  const catalog = useQuery({
    queryKey: ["series", project, mode],
    queryFn: () =>
      request<Series[]>(`${projectPath(project)}/history/catalog?mode=${mode}`),
    enabled: !!project,
    refetchInterval: active ? 5000 : false,
  });
  const samples = history.data ?? [];
  const rows = catalog.data ?? [];
  const orders = plotConfig
    ? {
        mv: plotConfig.mv_order,
        cv: plotConfig.cv_order,
        dv: plotConfig.dv_order,
      }
    : { mv: [], cv: [], dv: [] };
  async function persist(next: PlotConfig) {
    setPlotConfig(next);
    try {
      await act(`${projectPath(project)}/plots/config`, next, "PUT");
    } catch {
      /* Keep entered settings visible after an error. */
    }
  }
  if (!plotConfig) return null;
  function move(kind: "mv" | "cv" | "dv", name: string, direction: number) {
    const next = structuredClone(plotConfig!);
    const key = `${kind}_order` as const;
    const values = next[key];
    const index = values.indexOf(name);
    const target = index + direction;
    if (target < 0 || target >= values.length) return;
    [values[index], values[target]] = [values[target], values[index]];
    void persist(next);
  }
  return (
    <>
      <div className="toolbar">
        <div className="inline">
          <Field label="History source">
            <Select
              value={mode}
              onChange={(event) => setMode(event.target.value as Mode)}
            >
              <option value="runtime">Runtime</option>
              <option value="simulation">Simulation · seconds</option>
            </Select>
          </Field>
          <Field label="Sliding history (s)">
            <NumberInput
              value={plotConfig.window_sec}
              onChange={(value) =>
                setPlotConfig({ ...plotConfig, window_sec: value ?? 180 })
              }
              onBlur={() => void persist(plotConfig)}
            />
          </Field>
          <Field label="Future horizon (s)">
            <NumberInput
              value={plotConfig.future_window_sec}
              onChange={(value) =>
                setPlotConfig({ ...plotConfig, future_window_sec: value ?? 0 })
              }
              onBlur={() => void persist(plotConfig)}
            />
          </Field>
        </div>
        <Badge tone={active ? "good" : "neutral"}>
          {active ? "Following active execution" : "Saved project history"}
        </Badge>
      </div>
      {history.error && <Notice error>{history.error.message}</Notice>}
      <div className="tabs">
        {(["mv", "cv", "dv", "custom"] as const).map((kind) => (
          <Button
            key={kind}
            variant={group === kind ? "default" : "ghost"}
            onClick={() => setGroup(kind)}
          >
            {kind === "custom" ? "Custom plots" : kind.toUpperCase()}
          </Button>
        ))}
      </div>
      <div className="chart-grid">
        {group !== "custom" &&
          orders[group]
            .filter((name) => selected[name] !== false)
            .slice(0, compact ? 1 : undefined)
            .map((name) => {
              const series = rows.filter(
                (row) =>
                  row.signal.startsWith(`${group}.${name}.`) ||
                  row.signal.startsWith(`io.${group}.${name}.`),
              );
              // Absolute CV LL/HL describe optimization safety. Keeping them out
              // of ordinary trends also avoids flattening the measured CV scale.
              const visibleSeries =
                group === "cv"
                  ? series.filter(
                      (row) =>
                        !/^io\.cv\..*\.(ll|hl)$/.test(row.signal) &&
                        !/\.band_(low|high)$/.test(row.signal),
                    )
                  : series;
              const data = [
                ...makeTraces(samples, visibleSeries, mode),
                ...(group === "cv" || group === "mv"
                  ? futureTraces(
                      samples,
                      group,
                      name,
                      mode,
                      saved.data?.config.logic.cycle_ms ?? 1000,
                      plotConfig.future_window_sec,
                    )
                  : []),
              ];
              return (
                <section className="panel chart-panel" key={name}>
                  <div className="section-title">
                    <strong>
                      {group.toUpperCase()} · {name}
                    </strong>
                    {!compact && (
                      <div className="inline">
                        <Button
                          variant="ghost"
                          size="small"
                          aria-label={`Move ${name} earlier`}
                          onClick={() => move(group, name, -1)}
                        >
                          ↑
                        </Button>
                        <Button
                          variant="ghost"
                          size="small"
                          aria-label={`Move ${name} later`}
                          onClick={() => move(group, name, 1)}
                        >
                          ↓
                        </Button>
                      </div>
                    )}
                  </div>
                  <Plot
                    data={data}
                    title={name}
                    revision={`${project}:${mode}:${name}`}
                    height={compact ? 260 : 330}
                  />
                </section>
              );
            })}
        {group === "custom" &&
          plotConfig.custom_plots.map((plot, index) => (
            <section className="panel chart-panel" key={index}>
              <div className="section-title">
                <strong>{plot.title}</strong>
                <Button
                  variant="ghost"
                  size="small"
                  onClick={() => {
                    const next = structuredClone(plotConfig);
                    next.custom_plots.splice(index, 1);
                    void persist(next);
                  }}
                >
                  Remove plot
                </Button>
              </div>
              <Plot
                title={plot.title}
                revision={`${project}:${mode}:custom:${plot.title}`}
                data={makeTraces(
                  samples,
                  plot.traces.map((trace) => ({
                    ...trace,
                    value_kind:
                      rows.find(
                        (row) =>
                          row.direction === trace.direction &&
                          row.signal === trace.signal,
                      )?.value_kind ?? "f64",
                  })),
                  mode,
                )}
              />
            </section>
          ))}
      </div>
      {!samples.length && (
        <Notice>
          No recorded samples for this source. Start execution or select another
          history source.
        </Notice>
      )}
      {!compact && group !== "custom" && (
        <section className="panel">
          <h2>Visible {group.toUpperCase()} signals</h2>
          <div className="inline">
            {orders[group].map((name) => (
              <label key={name} className="check-label">
                <input
                  type="checkbox"
                  checked={selected[name] !== false}
                  onChange={(event) =>
                    setSelected((previous) => ({
                      ...previous,
                      [name]: event.target.checked,
                    }))
                  }
                />
                {name}
              </label>
            ))}
          </div>
        </section>
      )}
      {group === "custom" && (
        <section className="panel">
          <h2>Create multi-trace plot</h2>
          <Field label="Plot title">
            <Input
              value={customTitle}
              onChange={(event) => setCustomTitle(event.target.value)}
            />
          </Field>
          <div className="series-picker">
            {rows.map((row) => {
              const key = `${row.direction}::${row.signal}`;
              return (
                <label key={key} className="check-label">
                  <input
                    type="checkbox"
                    checked={customSeries.includes(key)}
                    onChange={(event) =>
                      setCustomSeries((previous) =>
                        event.target.checked
                          ? [...previous, key]
                          : previous.filter((value) => value !== key),
                      )
                    }
                  />
                  {row.signal}
                  <small>{row.direction}</small>
                </label>
              );
            })}
          </div>
          <Button
            disabled={!customTitle.trim() || !customSeries.length}
            onClick={() => {
              const next = structuredClone(plotConfig);
              next.custom_plots.push({
                title: customTitle,
                traces: customSeries.map((key) => {
                  const [direction, signal] = key.split("::");
                  return { direction, signal };
                }),
              });
              void persist(next);
              setCustomTitle("");
              setCustomSeries([]);
            }}
          >
            Save custom plot
          </Button>
        </section>
      )}
    </>
  );
}
function OnlineParameters() {
  const { project, execution, workspace, act } = useWorkspace();
  const active = execution?.snapshot.project_name === project;
  const params = useQuery({
    queryKey: ["online-params", project, execution?.mode],
    queryFn: () => request<Parameters>("/api/execution/params"),
    enabled: active,
    refetchInterval: 1500,
  });
  const [draft, setDraft] = useState<Parameters>({});
  const [search, setSearch] = useState("");
  useEffect(() => setDraft({}), [project, execution?.mode]);
  const locked = workspace.data?.online_optimization?.phase === "running";
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
            {Object.entries(params.data ?? {})
              .filter(([key]) => key.includes(search))
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([key, value]) => (
                <tr key={key}>
                  <th>{key}</th>
                  <td>{String(value)}</td>
                  <td>
                    {typeof value === "boolean" ? (
                      <Select
                        aria-label={key}
                        disabled={locked}
                        value={String(draft[key] ?? value)}
                        onChange={(event) =>
                          setDraft((previous) => ({
                            ...previous,
                            [key]: event.target.value === "true",
                          }))
                        }
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
                        onChange={(next) =>
                          setDraft((previous) => ({
                            ...previous,
                            [key]: next ?? 0,
                          }))
                        }
                      />
                    )}
                  </td>
                  <td>
                    <Button
                      size="small"
                      disabled={locked || draft[key] === undefined}
                      onClick={() =>
                        void act("/api/execution/parameters", {
                          project_name: project,
                          mode: execution.mode,
                          params: { [key]: draft[key] },
                        })
                          .then(() => {
                            setDraft((previous) => {
                              const next = { ...previous };
                              delete next[key];
                              return next;
                            });
                            void params.refetch();
                          })
                          .catch(() => {})
                      }
                    >
                      Apply online
                    </Button>
                  </td>
                </tr>
              ))}
          </tbody>
        </table>
      </div>
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
        best-known values. CV LL/HL are absolute safety boundaries.
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
                  <th>Low</th>
                  <th>High</th>
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
