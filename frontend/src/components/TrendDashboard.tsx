import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { CSSProperties } from "react";
import type { Data } from "plotly.js";
import {
  type DashboardPanel,
  type PlotConfig,
  type Mode,
  type Series,
  type HistoryWindow,
  projectPath,
  request,
} from "../api";
import { useWorkspace } from "../workspace";
import { Button, Badge, Field, Input, Notice, NumberInput, Select } from "./ui";
import { Plot } from "./Plot";
import {
  allocateTraceColors,
  canonicalSignal,
  colorTraces,
  cvBandTraces,
  futureTraces,
  makeTraces,
} from "../charts";

const emptyPanel = (): DashboardPanel => ({
  id: crypto.randomUUID(),
  kind: "cv",
  name: "",
  title: "",
  traces: [],
});
const layoutPresets = ["1x1", "2x2", "2x5", "3x2", "3x3"];

/** Presentation preferences are project scoped; execution and history remain owned by Rust. */
export function TrendDashboard({ compact = false }: { compact?: boolean }) {
  const { project, saved, execution, act } = useWorkspace();
  const [mode, setMode] = useState<Mode>("runtime");
  const [config, setConfig] = useState<PlotConfig>();
  const [page, setPage] = useState(0);
  const [editing, setEditing] = useState<DashboardPanel>();
  const [search, setSearch] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const dirty = useRef(false);
  const colorRegistry = useRef<Record<string, string>>({});
  const active = execution?.snapshot.project_name === project;
  useEffect(() => {
    if (active && execution) setMode(execution.mode);
  }, [active, execution?.mode]);
  useEffect(() => {
    dirty.current = false;
    colorRegistry.current = {};
    setConfig(undefined);
    setPage(0);
    setEditing(undefined);
    setError("");
  }, [project]);
  useEffect(() => {
    if (!dirty.current) setConfig(saved.data?.config.plots);
  }, [project, saved.data?.config.plots]);
  const history = useQuery({
    queryKey: [
      "history",
      project,
      mode,
      config?.window_sec,
      active ? execution?.snapshot.execution_id : "saved",
    ],
    queryFn: () =>
      request<HistoryWindow>(
        `${projectPath(project)}/history?mode=${mode}&window_ms=${(config?.window_sec ?? 180) * 1000}`,
      ),
    enabled: !!project,
    refetchInterval: active ? 1500 : false,
  });
  const catalog = useQuery({
    queryKey: [
      "series",
      project,
      mode,
      active ? execution?.snapshot.execution_id : "saved",
    ],
    queryFn: () =>
      request<Series[]>(`${projectPath(project)}/history/catalog?mode=${mode}`),
    enabled: !!project,
    refetchInterval: active ? 5000 : false,
  });
  const rows = useMemo(() => {
    const distinct = new Map<string, Series>();
    for (const row of catalog.data ?? []) {
      const signal = canonicalSignal(row.signal);
      distinct.set(`${row.direction}::${signal}`, { ...row, signal });
    }
    return [...distinct.values()];
  }, [catalog.data]);
  const dashboard = config?.dashboard ?? { columns: 2, rows: 2, panels: [] };
  const samples = history.data?.samples ?? [];
  const traces = dashboard.panels.map((panel): Data[] => {
    const selected =
      panel.kind === "custom"
        ? panel.traces.map((trace) => ({
            ...trace,
            value_kind:
              rows.find(
                (row) =>
                  row.direction === trace.direction &&
                  row.signal === trace.signal,
              )?.value_kind ?? "f64",
          }))
        : rows.filter((row) => {
            const prefix = `${panel.kind}.${panel.name}.`;
            if (panel.kind === "cv")
              return (
                ["process_value", "setpoint"].some(
                  (suffix) => row.signal === prefix + suffix,
                ) ||
                row.signal === `mpc.target.cv.${panel.name}.low` ||
                row.signal === `mpc.target.cv.${panel.name}.high`
              );
            return (
              row.signal.startsWith(prefix) ||
              row.signal.startsWith(`io.${prefix}`)
            );
          });
    return [
      ...makeTraces(samples, selected, mode),
      ...(panel.kind === "cv"
        ? (() => {
            const cv = saved.data?.config.io_definition.cvs.find(
              (row) => row.name === panel.name,
            );
            return cv
              ? cvBandTraces(
                  samples,
                  cv,
                  saved.data?.config.logic.params ?? {},
                  mode,
                )
              : [];
          })()
        : []),
      ...(panel.kind === "cv" || panel.kind === "mv"
        ? futureTraces(
            samples,
            panel.kind,
            panel.name,
            mode,
            saved.data?.config.logic.cycle_ms ?? 1000,
            config?.future_window_sec ?? 0,
          )
        : []),
    ];
  });
  const colors = allocateTraceColors(traces.flat(), {
    ...config?.trace_colors,
    ...colorRegistry.current,
  });
  colorRegistry.current = colors;
  const capacity = dashboard.columns * dashboard.rows;
  const pageCount = Math.max(1, Math.ceil(dashboard.panels.length / capacity));
  const currentPage = Math.min(page, pageCount - 1);
  function change(next: PlotConfig) {
    dirty.current = true;
    setConfig(next);
  }
  async function persist() {
    if (!config) return;
    const submitted = config;
    const name = project;
    setPending(true);
    setError("");
    try {
      const normalized = (await act(
        `${projectPath(name)}/plots/config`,
        { ...submitted, trace_colors: colors },
        "PUT",
      )) as PlotConfig;
      // Do not replace edits made while this save was in flight.
      setConfig((latest) => {
        if (latest === submitted) {
          dirty.current = false;
          return normalized;
        }
        return latest;
      });
    } catch (reason) {
      setError((reason as Error).message);
    } finally {
      setPending(false);
    }
  }
  if (!config) return null;
  const updateDashboard = (next: typeof dashboard) =>
    change({ ...config, dashboard: next });
  function move(index: number, direction: number) {
    const panels = [...dashboard.panels];
    const target = index + direction;
    if (target < 0 || target >= panels.length) return;
    [panels[index], panels[target]] = [panels[target], panels[index]];
    updateDashboard({ ...dashboard, panels });
  }
  const candidates =
    editing?.kind === "custom"
      ? rows
      : (saved.data?.config.variables[
          `${editing?.kind ?? "cv"}s` as "mvs" | "cvs" | "dvs"
        ] ?? []);
  return (
    <>
      <div className="toolbar dashboard-toolbar">
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
              value={config.window_sec}
              onChange={(value) =>
                change({ ...config, window_sec: value ?? 180 })
              }
            />
          </Field>
          <Field label="Future horizon (s)">
            <NumberInput
              value={config.future_window_sec}
              onChange={(value) =>
                change({ ...config, future_window_sec: value ?? 0 })
              }
            />
          </Field>
          {!compact && (
            <>
              <Field label="Dashboard layout">
                <Select
                  value={
                    layoutPresets.includes(
                      `${dashboard.columns}x${dashboard.rows}`,
                    )
                      ? `${dashboard.columns}x${dashboard.rows}`
                      : "custom"
                  }
                  onChange={(event) => {
                    if (event.target.value === "custom") return;
                    const [columns, rows] = event.target.value
                      .split("x")
                      .map(Number);
                    updateDashboard({ ...dashboard, columns, rows });
                  }}
                >
                  {layoutPresets.map((layout) => (
                    <option key={layout} value={layout}>
                      {layout.replace("x", " × ")}
                    </option>
                  ))}
                  <option value="custom">Custom</option>
                </Select>
              </Field>
              <Field label="Columns">
                <NumberInput
                  value={dashboard.columns}
                  onChange={(value) =>
                    updateDashboard({
                      ...dashboard,
                      columns: Math.max(1, Math.min(6, Math.round(value ?? 2))),
                    })
                  }
                />
              </Field>
              <Field label="Rows">
                <NumberInput
                  value={dashboard.rows}
                  onChange={(value) =>
                    updateDashboard({
                      ...dashboard,
                      rows: Math.max(1, Math.min(10, Math.round(value ?? 2))),
                    })
                  }
                />
              </Field>
            </>
          )}
        </div>
        <div className="inline">
          <Badge tone={active ? "good" : "neutral"}>
            {active ? "Following active execution" : "Saved project history"}
          </Badge>
          {!compact && (
            <>
              <Button
                variant="outline"
                onClick={() => {
                  setEditing(emptyPanel());
                  setSearch("");
                }}
              >
                Add plot
              </Button>
              <Button disabled={pending} onClick={() => void persist()}>
                {pending ? "Saving…" : "Save dashboard"}
              </Button>
            </>
          )}
        </div>
      </div>
      {dirty.current && !compact && (
        <p className="muted">
          Dashboard changes are unsaved. Save to keep this layout, plots, and
          trace colors for this project.
        </p>
      )}
      {(error || history.error) && (
        <Notice error>{error || history.error?.message}</Notice>
      )}
      {history.data?.downsampled && (
        <p className="muted">
          Displaying {samples.length.toLocaleString()} of{" "}
          {history.data.original_count.toLocaleString()} stored samples,
          preserving extrema.
        </p>
      )}
      {!compact && (
        <div
          className="dashboard-pagination inline"
          aria-label="Dashboard pages"
        >
          <Button
            variant="ghost"
            disabled={currentPage === 0}
            onClick={() => setPage(currentPage - 1)}
          >
            Previous page
          </Button>
          <span>
            Page {currentPage + 1} of {pageCount} · {dashboard.panels.length}{" "}
            plots · columns × rows
          </span>
          <Button
            variant="ghost"
            disabled={currentPage + 1 >= pageCount}
            onClick={() => setPage(currentPage + 1)}
          >
            Next page
          </Button>
        </div>
      )}
      <div
        className={`dashboard-grid ${compact ? "dashboard-compact" : ""}`}
        style={
          {
            "--dashboard-columns": compact ? 1 : dashboard.columns,
          } as CSSProperties
        }
      >
        {dashboard.panels
          .slice(
            compact ? 0 : currentPage * capacity,
            compact ? 1 : (currentPage + 1) * capacity,
          )
          .map((panel, localIndex) => {
            const index = compact ? 0 : currentPage * capacity + localIndex;
            return (
              <section
                className="panel chart-panel"
                key={`${project}:${panel.id}`}
              >
                <div className="section-title">
                  <strong>
                    {panel.kind === "custom"
                      ? panel.title
                      : `${panel.kind.toUpperCase()} · ${panel.title || panel.name}`}
                  </strong>
                  {!compact && (
                    <div className="inline">
                      <Button
                        variant="ghost"
                        size="small"
                        aria-label={`Move ${panel.title} earlier`}
                        disabled={index === 0}
                        onClick={() => move(index, -1)}
                      >
                        ↑
                      </Button>
                      <Button
                        variant="ghost"
                        size="small"
                        aria-label={`Move ${panel.title} later`}
                        disabled={index === dashboard.panels.length - 1}
                        onClick={() => move(index, 1)}
                      >
                        ↓
                      </Button>
                      <Button
                        variant="ghost"
                        size="small"
                        onClick={() => {
                          setEditing(structuredClone(panel));
                          setSearch("");
                        }}
                      >
                        Edit plot
                      </Button>
                      <Button
                        variant="ghost"
                        size="small"
                        onClick={() =>
                          updateDashboard({
                            ...dashboard,
                            panels: dashboard.panels.filter(
                              (item) => item.id !== panel.id,
                            ),
                          })
                        }
                      >
                        Remove plot
                      </Button>
                    </div>
                  )}
                </div>
                <Plot
                  data={colorTraces(traces[index], colors)}
                  title={panel.title || panel.name}
                  revision={`${project}:${mode}:${panel.id}`}
                  height={compact ? 260 : 330}
                />
              </section>
            );
          })}
        {!compact &&
          Array.from(
            {
              length: Math.max(
                0,
                capacity - (dashboard.panels.length - currentPage * capacity),
              ),
            },
            (_, slot) => (
              <button
                className="dashboard-empty"
                key={`empty-${slot}`}
                aria-label={`Add plot to slot ${dashboard.panels.length + slot + 1}`}
                onClick={() => {
                  setEditing(emptyPanel());
                  setSearch("");
                }}
              >
                + Add plot
              </button>
            ),
          )}
      </div>
      {!samples.length && (
        <Notice>
          No recorded samples for this source. Start execution or select another
          history source.
        </Notice>
      )}
      {editing && !compact && (
        <section className="panel plot-editor" aria-label="Plot editor">
          <h2>
            {dashboard.panels.some((panel) => panel.id === editing.id)
              ? "Edit plot"
              : "Add plot"}
          </h2>
          <div className="form-grid">
            <Field label="Plot type">
              <Select
                aria-label="Plot type"
                value={editing.kind}
                onChange={(event) => {
                  setEditing({
                    ...editing,
                    kind: event.target.value as DashboardPanel["kind"],
                    name: "",
                    traces: [],
                  });
                  setSearch("");
                }}
              >
                <option value="cv">CV</option>
                <option value="mv">MV</option>
                <option value="dv">DV</option>
                <option value="custom">Custom multi-trace</option>
              </Select>
            </Field>
            <Field label="Plot title">
              <Input
                value={editing.title}
                onChange={(event) =>
                  setEditing({ ...editing, title: event.target.value })
                }
              />
            </Field>
            <Field label="Search plot signals">
              <Input
                value={search}
                onChange={(event) => setSearch(event.target.value)}
              />
            </Field>
          </div>
          <div className="series-picker">
            {candidates
              .filter((item) =>
                (typeof item === "string"
                  ? item
                  : `${item.signal} ${item.direction}`
                )
                  .toLowerCase()
                  .includes(search.toLowerCase()),
              )
              .map((item) => {
                if (typeof item === "string")
                  return (
                    <label className="check-label" key={item}>
                      <input
                        type="radio"
                        name="plot-variable"
                        checked={editing.name === item}
                        onChange={() =>
                          setEditing({
                            ...editing,
                            name: item,
                            title: editing.title || item,
                          })
                        }
                      />
                      {item}
                    </label>
                  );
                const checked = editing.traces.some(
                  (trace) =>
                    trace.direction === item.direction &&
                    trace.signal === item.signal,
                );
                return (
                  <label
                    className="check-label"
                    key={`${item.direction}::${item.signal}`}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(event) =>
                        setEditing({
                          ...editing,
                          traces: event.target.checked
                            ? [
                                ...editing.traces,
                                {
                                  direction: item.direction,
                                  signal: item.signal,
                                },
                              ]
                            : editing.traces.filter(
                                (trace) =>
                                  trace.direction !== item.direction ||
                                  trace.signal !== item.signal,
                              ),
                        })
                      }
                    />
                    {item.signal}
                    <small>{item.direction}</small>
                  </label>
                );
              })}
          </div>
          {!candidates.length && (
            <p className="muted">
              No signals available. Configure variables or record execution
              history first.
            </p>
          )}
          <div className="inline">
            <Button
              disabled={
                !editing.title.trim() ||
                (editing.kind === "custom"
                  ? !editing.traces.length
                  : !editing.name)
              }
              onClick={() => {
                const existing = dashboard.panels.findIndex(
                  (panel) => panel.id === editing.id,
                );
                const panels = [...dashboard.panels];
                if (existing >= 0) panels[existing] = editing;
                else panels.push(editing);
                updateDashboard({ ...dashboard, panels });
                setPage(
                  Math.floor(
                    (existing >= 0 ? existing : panels.length - 1) / capacity,
                  ),
                );
                setEditing(undefined);
              }}
            >
              Apply plot
            </Button>
            <Button variant="ghost" onClick={() => setEditing(undefined)}>
              Cancel
            </Button>
          </div>
        </section>
      )}
    </>
  );
}
