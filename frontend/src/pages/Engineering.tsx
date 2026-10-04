import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router-dom";
import {
  type Job,
  type NumericRange,
  type ProjectConfig,
  download,
  projectPath,
  request,
} from "../api";
import { useWorkspace } from "../workspace";
import {
  Button,
  Field,
  Input,
  Notice,
  NumberInput,
  Select,
  Badge,
} from "../components/ui";
import { MatrixEditor } from "../components/MatrixEditor";
import { Plot } from "../components/Plot";
import type { Data } from "plotly.js";

export function Engineering() {
  const { section = "simulation" } = useParams();
  const { config, jobs, project } = useWorkspace();
  if (!config) return null;
  return (
    <>
      <div className="page-title">
        <div className="eyebrow">Engineering · {project}</div>
        <h1>
          {{
            simulation: "Controller simulation",
            tests: "Control function tests",
            kalman: "Kalman observer replay",
            optimization: "Offline optimization",
          }[section] || section}
        </h1>
        <p>Evaluate saved configuration with the existing Rust algorithms.</p>
      </div>
      {section === "simulation" ? (
        <Simulation />
      ) : section === "tests" ? (
        <ControlTests />
      ) : section === "kalman" ? (
        <Kalman />
      ) : (
        <OfflineOptimization />
      )}
      <section className="panel">
        <h2>Engineering jobs</h2>
        <p className="muted">
          Jobs continue when you navigate or close the browser. Results are
          saved with the project.
        </p>
        {jobs.error && <Notice error>{jobs.error.message}</Notice>}
        {jobs.data?.length ? (
          jobs.data.map((job) => <JobResult key={job.id} job={job} />)
        ) : (
          <p className="empty">No jobs yet.</p>
        )}
      </section>
    </>
  );
}
function Simulation() {
  const {
    config: cfg,
    edit,
    execution,
    project,
    dirty,
    locked,
    act,
    catalog,
    workspace,
    acceptSavedSection,
  } = useWorkspace();
  if (!cfg) return null;
  const active =
    execution?.mode === "simulation" &&
    execution.snapshot.project_name === project;
  const simulation = cfg.simulation;
  return (
    <>
      <div className="toolbar">
        <div className="inline">
          <Button
            disabled={
              !!execution || dirty || !catalog.data?.simulation_available
            }
            onClick={() =>
              void act("/api/execution/command", {
                action: "start",
                mode: "simulation",
                execution_id: execution?.snapshot.execution_id,
                project_name: project,
              }).catch(() => {})
            }
          >
            Start saved simulation
          </Button>
          <Button
            variant="destructive"
            disabled={!active}
            onClick={() =>
              void act("/api/execution/command", {
                action: "stop",
                mode: "simulation",
                execution_id: execution?.snapshot.execution_id,
                project_name: project,
              }).catch(() => {})
            }
          >
            Stop simulation
          </Button>
          {active && (
            <Badge tone="good">
              {execution.snapshot.simulation_time_s?.toFixed(2)} s simulated
            </Badge>
          )}
        </div>
        <Button
          variant="outline"
          disabled={
            !active || workspace.data?.online_optimization?.phase === "running"
          }
          onClick={() =>
            void act("/api/execution/simulation-config", {
              project_name: project,
              mode: "simulation",
              execution_id: execution?.snapshot.execution_id,
              config: simulation,
            })
              .then(() => acceptSavedSection("simulation"))
              .catch(() => {})
          }
        >
          Apply simulation settings live
        </Button>
      </div>
      {catalog.data?.simulation_issue && (
        <Notice>{catalog.data.simulation_issue}</Notice>
      )}
      <section className="panel">
        <h2>Execution speed</h2>
        <Field label="Simulation speed">
          <Select
            value={simulation.speed}
            onChange={(event) =>
              edit((next) => {
                next.simulation.speed = event.target.value;
              })
            }
          >
            {[
              "real_time",
              "x0_1",
              "x0_5",
              "x10",
              "x20",
              "x50",
              "x100",
              "as_fast_as_possible",
            ].map((speed) => (
              <option key={speed} value={speed}>
                {speed.replaceAll("_", " ")}
              </option>
            ))}
          </Select>
        </Field>
      </section>
      <div className="editor-grid">
        {(
          [
            "cv_setpoints",
            "cv_initials",
            "mv_initials",
            "dv_initials",
            "cv_noise_std",
            "mv_noise_std",
            "dv_disturbances",
            "cv_offsets",
          ] as const
        ).map((group) => (
          <section key={group} className="panel">
            <h2>{group.replaceAll("_", " ")}</h2>
            <div className="form-grid">
              {Object.entries(simulation[group]).map(([key, value]) => (
                <Field key={key} label={key}>
                  <NumberInput
                    value={value}
                    onChange={(next) =>
                      edit((cfg) => {
                        cfg.simulation[group][key] = next ?? 0;
                      })
                    }
                  />
                </Field>
              ))}
            </div>
            {!Object.keys(simulation[group]).length && (
              <p className="muted">No applicable signals.</p>
            )}
          </section>
        ))}
      </div>
      <div className="matrix-grid">
        {(["a_scale_pct", "b_scale_pct"] as const).map((key) => (
          <MatrixEditor
            key={key}
            name={`${key[0].toUpperCase()} scaling (%)`}
            matrix={simulation[key]}
            cols={key === "b_scale_pct" ? cfg.model?.u_order : undefined}
            onChange={(matrix) =>
              edit((next) => {
                next.simulation[key] = matrix;
              })
            }
          />
        ))}
      </div>
      <p className="muted">
        Initial states apply at the next start. Speed, disturbances, noise and
        A/B scales can be applied to the active simulation.
      </p>
    </>
  );
}
function ControlTests() {
  const { catalog, startJob, locked, dirty, project } = useWorkspace();
  const [selected, setSelected] = useState<
    Record<
      string,
      { enabled: boolean; min: number; max: number; samples: number }
    >
  >({});
  useEffect(() => setSelected({}), [project]);
  const dimensions = catalog.data?.sweeps ?? [];
  return (
    <section className="panel">
      <div className="section-title">
        <h2>Input and parameter sweeps</h2>
        <Button
          disabled={locked || dirty}
          onClick={() =>
            void startJob({
              kind: "control_tests",
              selections: dimensions.map((row) => {
                const values = selected[`${row.kind}:${row.name}`];
                return {
                  kind: row.kind,
                  name: row.name,
                  enabled: values?.enabled ?? false,
                  numeric:
                    row.value_kind === "F64"
                      ? {
                          min: values?.min ?? row.default_min,
                          max: values?.max ?? row.default_max,
                          samples: values?.samples ?? row.default_samples,
                        }
                      : null,
                };
              }),
            }).catch(() => {})
          }
        >
          Run control tests
        </Button>
      </div>
      <p className="muted">
        Full factorial evaluation falls back to single-axis sweeps at the
        existing case limit. Results include timing, output validation and case
        previews.
      </p>
      <div className="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Include</th>
              <th>Dimension</th>
              <th>Baseline</th>
              <th>Min</th>
              <th>Max</th>
              <th>Samples</th>
            </tr>
          </thead>
          <tbody>
            {dimensions.map((row) => {
              const key = `${row.kind}:${row.name}`;
              const values = selected[key] ?? {
                enabled: false,
                min: row.default_min ?? 0,
                max: row.default_max ?? 0,
                samples: row.default_samples,
              };
              return (
                <tr key={key}>
                  <td>
                    <input
                      type="checkbox"
                      aria-label={`Sweep ${row.name}`}
                      checked={values.enabled}
                      onChange={(event) =>
                        setSelected((previous) => ({
                          ...previous,
                          [key]: { ...values, enabled: event.target.checked },
                        }))
                      }
                    />
                  </td>
                  <th>
                    <small>{row.kind}</small>
                    <br />
                    {row.name}
                  </th>
                  <td>{row.baseline_label}</td>
                  {row.value_kind === "F64" ? (
                    (["min", "max", "samples"] as const).map((field) => (
                      <td key={field}>
                        <NumberInput
                          aria-label={`${row.name} ${field}`}
                          value={values[field]}
                          onChange={(value) =>
                            setSelected((previous) => ({
                              ...previous,
                              [key]: { ...values, [field]: value ?? 0 },
                            }))
                          }
                        />
                      </td>
                    ))
                  ) : (
                    <td colSpan={3}>False / true</td>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
function Kalman() {
  const { catalog, project, edit, startJob, locked, dirty, save } =
    useWorkspace();
  const [upload, setUpload] = useState<{ file_name: string; csv: string }>();
  const [parameters, setParameters] = useState<Record<string, number>>({});
  useEffect(() => {
    setParameters(catalog.data?.kalman_parameters ?? {});
    setUpload(undefined);
  }, [project, JSON.stringify(catalog.data?.kalman_parameters)]);
  return (
    <section className="panel">
      <h2>Historical measurements</h2>
      {!catalog.data?.kalman_parameters ? (
        <Notice>
          A compatible MPC or custom observer schema and linear model are
          required. Custom aliases kalman.* and observer.* are supported.
        </Notice>
      ) : (
        <>
          <p className="muted">
            Upload UTF-8 CSV with a time-zone-aware time column, strictly
            increasing times, every configured CV/MV/DV, and at least two finite
            rows.
          </p>
          <input
            type="file"
            aria-label="Kalman CSV"
            accept=".csv"
            onChange={(event) => {
              const file = event.target.files?.[0];
              if (file)
                void file
                  .text()
                  .then((csv) => setUpload({ file_name: file.name, csv }));
            }}
          />
          {upload && <Notice>{upload.file_name} · ready to replay</Notice>}
          <div className="form-grid">
            {Object.entries(parameters).map(([key, value]) => (
              <Field key={key} label={key}>
                <NumberInput
                  value={value}
                  onChange={(next) =>
                    setParameters((previous) => ({
                      ...previous,
                      [key]: next ?? 0,
                    }))
                  }
                />
              </Field>
            ))}
          </div>
          <div className="inline">
            <Button
              disabled={locked || dirty || !upload}
              onClick={() =>
                void startJob({
                  kind: "kalman_replay",
                  ...upload,
                  parameters,
                }).catch(() => {})
              }
            >
              Run observer
            </Button>
            <Button
              variant="outline"
              disabled={locked}
              onClick={() =>
                edit((next) => {
                  Object.assign(next.logic.params, parameters);
                })
              }
            >
              Stage observer parameters for save
            </Button>
          </div>
        </>
      )}
    </section>
  );
}
export function RangeTable({
  parameters,
  onChange,
  online = false,
}: {
  parameters: {
    key: string;
    min: number;
    max: number;
    start?: number;
    fixed?: boolean;
  }[];
  onChange: (index: number, key: string, value: number | boolean) => void;
  online?: boolean;
}) {
  return (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th>Numeric parameter</th>
            {!online && (
              <>
                <th>Fixed</th>
                <th>Start</th>
              </>
            )}
            <th>Min</th>
            <th>Max</th>
          </tr>
        </thead>
        <tbody>
          {parameters.map((row, index) => (
            <tr key={row.key}>
              <th>{row.key}</th>
              {!online && (
                <>
                  <td>
                    <input
                      type="checkbox"
                      aria-label={`${row.key} fixed`}
                      checked={row.fixed ?? false}
                      onChange={(event) =>
                        onChange(index, "fixed", event.target.checked)
                      }
                    />
                  </td>
                  <td>
                    <NumberInput
                      aria-label={`${row.key} start`}
                      value={row.start}
                      onChange={(value) => onChange(index, "start", value ?? 0)}
                    />
                  </td>
                </>
              )}
              {(["min", "max"] as const).map((key) => (
                <td key={key}>
                  <NumberInput
                    aria-label={`${row.key} ${key}`}
                    value={row[key]}
                    onChange={(value) => onChange(index, key, value ?? 0)}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
function OfflineOptimization() {
  const {
    config: cfg,
    edit,
    startJob,
    locked,
    dirty,
    catalog,
  } = useWorkspace();
  if (!cfg) return null;
  const opt = cfg.optimization;
  return (
    <>
      <Notice>
        Offline optimization uses a model and simulated scenarios. It is
        optional for deployment. Boolean L1/L2 choices and external CV band
        edges are excluded.
      </Notice>
      <section className="panel">
        <div className="section-title">
          <h2>Parameter search bounds</h2>
          <Button
            disabled={
              locked || dirty || !catalog.data?.offline_optimization_available
            }
            onClick={() =>
              void startJob({ kind: "offline_optimization" }).catch(() => {})
            }
          >
            Start saved optimization
          </Button>
        </div>
        <RangeTable
          parameters={opt.parameters}
          onChange={(index, key, value) =>
            edit((next) => {
              (
                next.optimization.parameters[index] as unknown as Record<
                  string,
                  unknown
                >
              )[key] = value;
            })
          }
        />
      </section>
      <section className="panel">
        <h2>Search and uncertainty</h2>
        <div className="form-grid">
          {(
            [
              "scenario_count",
              "max_iterations",
              "initial_samples",
              "convergence_patience",
              "plot_interval",
              "simulation_duration_s",
              "a_scale_min_pct",
              "a_scale_max_pct",
              "b_scale_min_pct",
              "b_scale_max_pct",
            ] as const
          ).map((key) => (
            <Field key={key} label={key.replaceAll("_", " ")}>
              <NumberInput
                value={opt[key]}
                onChange={(value) =>
                  edit((next) => {
                    next.optimization[key] = value ?? 0;
                  })
                }
              />
            </Field>
          ))}
        </div>
      </section>
      <div className="editor-grid">
        {(
          [
            "cv_target_steps",
            "cv_weights",
            "cv_noise_std",
            "mv_noise_std",
            "metric_weights",
          ] as const
        ).map((group) => (
          <section className="panel" key={group}>
            <h2>{group.replaceAll("_", " ")}</h2>
            <div className="form-grid">
              {Object.entries(opt[group]).map(([key, value]) => (
                <Field key={key} label={key}>
                  <NumberInput
                    value={value}
                    onChange={(next) =>
                      edit((cfg) => {
                        cfg.optimization[group][key] = next ?? 0;
                      })
                    }
                  />
                </Field>
              ))}
            </div>
          </section>
        ))}
      </div>
    </>
  );
}

function JobResult({ job: summary }: { job: Job }) {
  const { project, act, locked } = useWorkspace();
  const [open, setOpen] = useState(summary.phase === "running");
  const detail = useQuery({
    queryKey: ["job-detail", project, summary.id, summary.phase],
    queryFn: () => request<Job>(`${projectPath(project)}/jobs/${summary.id}`),
    enabled: open,
    refetchInterval: summary.phase === "running" && open ? 1500 : false,
  });
  const job = detail.data ?? summary;
  const result = job.result;
  const curves = (result?.curves ?? job.progress.curves) as
    | {
        cvs?: {
          cv: string;
          target: number;
          scenarios: { scenario_index: number; points: number[][] }[];
        }[];
        inputs?: {
          input: string;
          scenarios: { scenario_index: number; points: number[][] }[];
        }[];
      }
    | undefined;
  const convergence = (result?.convergence_history ??
    job.progress.convergence_history) as number[][] | undefined;
  return (
    <details
      className="job"
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <div>
          <strong>{job.kind.replaceAll("_", " ")}</strong>
          <small>
            {new Date(job.started_at).toLocaleString()} · {job.id.slice(0, 8)}
          </small>
        </div>
        <Badge
          tone={
            job.phase === "failed"
              ? "danger"
              : job.phase === "running"
                ? "accent"
                : "good"
          }
        >
          {job.phase}
        </Badge>
      </summary>
      <div className="job-body">
        {detail.isLoading && open && <p>Loading full job report…</p>}
        {detail.error && <Notice error>{detail.error.message}</Notice>}
        {job.phase === "running" && (
          <>
            <p>
              {String(job.progress.status ?? "Running")}{" "}
              {job.progress.iteration !== undefined &&
                `· ${job.progress.iteration} / ${job.progress.max_iterations}`}
            </p>
            {job.cancellable && (
              <Button
                variant="outline"
                onClick={() =>
                  void act(
                    `${projectPath(project)}/jobs/${job.id}/cancel`,
                  ).catch(() => {})
                }
              >
                Cancel optimization
              </Button>
            )}
          </>
        )}
        {job.error && <Notice error>{job.error}</Notice>}
        {result && (
          <>
            <div className="inline">
              <Button
                variant="outline"
                onClick={() =>
                  download(
                    `${job.kind}-${job.id}.json`,
                    JSON.stringify(job, null, 2),
                    "application/json",
                  )
                }
              >
                Download report
              </Button>
              {job.kind === "offline_optimization" && (
                <Button
                  disabled={locked}
                  onClick={() =>
                    void act(
                      `${projectPath(project)}/jobs/${job.id}/apply`,
                    ).catch(() => {})
                  }
                >
                  Apply best parameters to saved project
                </Button>
              )}
            </div>
            {job.kind === "strategy_build" ? (
              <>
                <Notice
                  error={
                    result.success === false || result.compatible === false
                  }
                >
                  {String(result.message)}
                </Notice>
                <pre>{String(result.output)}</pre>
              </>
            ) : job.kind === "kalman_replay" ? (
              <ReplayCharts result={result} />
            ) : job.kind === "control_tests" ? (
              <TestReport result={result} />
            ) : (
              <pre>
                {JSON.stringify(
                  result,
                  (key, value) =>
                    key === "curves" || key === "convergence_history"
                      ? undefined
                      : value,
                  2,
                )}
              </pre>
            )}
          </>
        )}
        {convergence?.length ? (
          <Plot
            title="Optimization convergence"
            data={[
              {
                x: convergence.map((row) => row[0]),
                y: convergence.map((row) => row[1]),
                name: "Best score",
                type: "scatter",
                mode: "lines+markers",
              },
            ]}
          />
        ) : null}
        {curves?.cvs?.map((cv) => (
          <Plot
            key={cv.cv}
            title={`CV ${cv.cv} · target ${cv.target}`}
            data={cv.scenarios.map((scenario) => ({
              x: scenario.points.map((row) => row[0]),
              y: scenario.points.map((row) => row[1]),
              name: `Scenario ${scenario.scenario_index + 1}`,
              type: "scatter",
              mode: "lines",
            }))}
          />
        ))}
        {curves?.inputs?.map((input) => (
          <Plot
            key={input.input}
            title={input.input}
            data={input.scenarios.map((scenario) => ({
              x: scenario.points.map((row) => row[0]),
              y: scenario.points.map((row) => row[1]),
              name: `Scenario ${scenario.scenario_index + 1}`,
              type: "scatter",
              mode: "lines",
            }))}
          />
        ))}
      </div>
    </details>
  );
}
function TestReport({ result }: { result: Record<string, unknown> }) {
  const execution = result.execution as Record<string, string | number>;
  return (
    <>
      <div className="metrics-grid">
        {Object.entries(execution).map(([key, value]) => (
          <div key={key}>
            <small>{key.replaceAll("_", " ")}</small>
            <strong>
              {typeof value === "number"
                ? value.toLocaleString(undefined, { maximumFractionDigits: 3 })
                : value}
            </strong>
          </div>
        ))}
      </div>
      <pre>
        {JSON.stringify(
          {
            counts: result.counts,
            outputs: result.outputs,
            slowest_cases: result.slowest_cases,
            issue_cases: result.issue_cases,
          },
          null,
          2,
        )}
      </pre>
    </>
  );
}
function ReplayCharts({ result }: { result: Record<string, unknown> }) {
  const timestamps = result.timestamps as string[];
  const cvs = result.cvs as {
    name: string;
    measurement: number[];
    estimate: number[];
  }[];
  const groups = ["mvs", "dvs", "states", "biases"];
  return (
    <>
      {cvs.map((cv) => (
        <Plot
          key={cv.name}
          title={`CV ${cv.name} · observer replay`}
          data={[
            {
              x: timestamps,
              y: cv.measurement,
              name: "Measurement",
              type: "scatter",
              mode: "lines",
            },
            {
              x: timestamps,
              y: cv.estimate,
              name: "Estimate",
              type: "scatter",
              mode: "lines",
            },
          ]}
        />
      ))}
      {groups.map((group) => (
        <Plot
          key={group}
          title={group}
          data={(result[group] as { name: string; values: number[] }[]).map(
            (series) => ({
              x: timestamps,
              y: series.values,
              name: series.name,
              type: "scatter",
              mode: "lines",
            }),
          )}
        />
      ))}
    </>
  );
}
