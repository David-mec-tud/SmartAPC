import { useState } from "react";
import { useParams } from "react-router-dom";
import {
  type Cv,
  type Dv,
  type Model,
  type Mv,
  download,
  projectPath,
  request,
  type ProjectConfig,
} from "../api";
import { useWorkspace } from "../workspace";
import {
  Button,
  Field,
  Input,
  Notice,
  NumberInput,
  Select,
} from "../components/ui";
import { MatrixEditor, resizeMatrix } from "../components/MatrixEditor";

export function Configuration() {
  const { section = "communication" } = useParams();
  const { config, edit, locked } = useWorkspace();
  if (!config) return null;
  return (
    <>
      <div className="page-title">
        <div className="eyebrow">Project configuration</div>
        <h1>
          {{
            communication: "Connection & timing",
            io: "Variables & I/O",
            model: "Plant model",
            strategy: "Control strategy",
            settings: "Project settings",
          }[section] || section}
        </h1>
        <p>
          Define and save the controller’s hardware and calculation contract.
        </p>
      </div>
      {locked && (
        <Notice>
          Configuration is locked while this project executes or an engineering
          job runs.
        </Notice>
      )}
      <fieldset disabled={locked} className="editor-fieldset">
        {section === "communication" ? (
          <Communication />
        ) : section === "io" ? (
          <Io />
        ) : section === "model" ? (
          <ModelEditor />
        ) : section === "strategy" ? (
          <Strategy />
        ) : (
          <Settings />
        )}
      </fieldset>
    </>
  );
}
function Communication() {
  const { config: cfg, edit, startJob, locked } = useWorkspace();
  if (!cfg) return null;
  return (
    <>
      <div className="editor-grid">
        <section className="panel">
          <h2>OPC UA connection</h2>
          <Field label="Endpoint URL">
            <Input
              value={cfg.communication.endpoint_url}
              onChange={(event) =>
                edit((next) => {
                  next.communication.endpoint_url = event.target.value;
                })
              }
            />
          </Field>
          <p className="muted">
            Connection and watchdog checks run in Rust independently of the
            browser.
          </p>
        </section>
        <section className="panel">
          <h2>Independent cycle periods</h2>
          <Field label="Communication / watchdog (ms)">
            <NumberInput
              value={cfg.communication.cycle_ms}
              onChange={(value) =>
                edit((next) => {
                  next.communication.cycle_ms = value ?? 1;
                })
              }
            />
          </Field>
          <Field
            label="I/O exchanges (ms)"
            hint="Must be a multiple of the communication cycle."
          >
            <NumberInput
              value={cfg.io.cycle_ms}
              onChange={(value) =>
                edit((next) => {
                  next.io.cycle_ms = value ?? 1;
                })
              }
            />
          </Field>
          <Field
            label="Controller calculations (ms)"
            hint="Also controls simulation and tuning discretization."
          >
            <NumberInput
              value={cfg.logic.cycle_ms}
              onChange={(value) =>
                edit((next) => {
                  next.logic.cycle_ms = value ?? 1;
                })
              }
            />
          </Field>
        </section>
      </div>
      <section className="panel">
        <h2>Watchdog</h2>
        <Field label="Mode">
          <Select
            value={cfg.watchdog.kind}
            onChange={(event) =>
              edit((next) => {
                next.watchdog =
                  event.target.value === "none"
                    ? { kind: "none" }
                    : {
                        kind: "counter_mirror",
                        read_node: "",
                        write_node: "",
                        stale_timeout_ms: 2000,
                      };
              })
            }
          >
            <option value="none">Disabled</option>
            <option value="counter_mirror">Counter mirror</option>
          </Select>
        </Field>
        {cfg.watchdog.kind === "counter_mirror" && (
          <div className="form-grid">
            {(["read_node", "write_node", "stale_timeout_ms"] as const).map(
              (key) => (
                <Field key={key} label={key.replaceAll("_", " ")}>
                  <Input
                    value={
                      cfg.watchdog.kind === "counter_mirror"
                        ? cfg.watchdog[key]
                        : ""
                    }
                    onChange={(event) =>
                      edit((next) => {
                        if (next.watchdog.kind === "counter_mirror") {
                          if (key === "stale_timeout_ms")
                            next.watchdog[key] = Number(event.target.value);
                          else next.watchdog[key] = event.target.value;
                        }
                      })
                    }
                  />
                </Field>
              ),
            )}
          </div>
        )}
        <Button
          variant="outline"
          disabled={locked}
          onClick={() =>
            void startJob({ kind: "watchdog_test" }).catch(() => {})
          }
        >
          Test saved watchdog · reads and mirrors counter
        </Button>
      </section>
    </>
  );
}
type IoRow = Mv | Cv | Dv;
const columns: Record<
  "mvs" | "cvs" | "dvs",
  { key: string; label: string; numeric?: boolean }[]
> = {
  mvs: [
    { key: "mv_write", label: "Write Node ID" },
    { key: "mv_read", label: "Read Node ID" },
    { key: "ll", label: "Initial LL", numeric: true },
    { key: "hl", label: "Initial HL", numeric: true },
    { key: "ll_opc", label: "LL Node ID" },
    { key: "hl_opc", label: "HL Node ID" },
    { key: "enable", label: "Enable Node ID" },
  ],
  cvs: [
    { key: "setpoint", label: "Setpoint Node ID" },
    { key: "process_value", label: "Process value Node ID" },
    { key: "ll", label: "Safety LL", numeric: true },
    { key: "hl", label: "Safety HL", numeric: true },
    { key: "band_low_value", label: "Band low offset", numeric: true },
    { key: "band_high_value", label: "Band high offset", numeric: true },
    { key: "band_low_opc", label: "Band low Node ID" },
    { key: "band_high_opc", label: "Band high Node ID" },
    { key: "deadband_value", label: "Legacy deadband", numeric: true },
    { key: "deadband_opc", label: "Deadband Node ID" },
    { key: "enable", label: "Enable Node ID" },
  ],
  dvs: [
    { key: "read", label: "Read Node ID" },
    { key: "enable", label: "Enable Node ID" },
  ],
};
function Io() {
  const { config: cfg, edit, startJob, locked, dirty } = useWorkspace();
  const [group, setGroup] = useState<"mvs" | "cvs" | "dvs">("mvs");
  const [newName, setNewName] = useState("");
  const [writeValues, setWriteValues] = useState<Record<string, number>>({});
  if (!cfg) return null;
  function update(index: number, key: string, value: string | number | null) {
    edit((next) => {
      const row = next.io_definition[group][index] as unknown as Record<
        string,
        unknown
      >;
      row[key] = value;
    });
  }
  function add() {
    if (!newName.trim()) return;
    edit((next) => {
      const name = newName.trim();
      next.variables[group].push(name);
      const row =
        group === "mvs"
          ? { name, mv_write: "", mv_read: "" }
          : group === "cvs"
            ? { name, setpoint: "", process_value: "" }
            : { name, read: "" };
      (next.io_definition[group] as IoRow[]).push(row);
    });
    setNewName("");
  }
  return (
    <>
      <Notice>
        CV bands are offsets from the setpoint. External band Node IDs own that
        edge. CV safety LL/HL apply only to online optimization.
      </Notice>
      <div className="toolbar">
        <div className="tabs">
          {(["mvs", "cvs", "dvs"] as const).map((key) => (
            <Button
              key={key}
              variant={group === key ? "default" : "ghost"}
              onClick={() => setGroup(key)}
            >
              {key.toUpperCase()} <span>{cfg.variables[key].length}</span>
            </Button>
          ))}
        </div>
        <div className="inline">
          <Input
            aria-label="New variable name"
            placeholder="Variable name"
            value={newName}
            onChange={(event) => setNewName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                add();
              }
            }}
          />
          <Button variant="outline" onClick={add}>
            Add variable
          </Button>
        </div>
      </div>
      <div className="panel table-scroll io-table">
        <table>
          <thead>
            <tr>
              <th>Variable</th>
              {columns[group].map((column) => (
                <th key={column.key}>{column.label}</th>
              ))}
              <th>Remove</th>
            </tr>
          </thead>
          <tbody>
            {cfg.io_definition[group].map((row, index) => (
              <tr key={`${group}-${index}`}>
                <th>{row.name}</th>
                {columns[group].map((column) => {
                  const value = (
                    row as unknown as Record<string, string | number | null>
                  )[column.key];
                  return (
                    <td key={column.key}>
                      {column.numeric ? (
                        <NumberInput
                          aria-label={`${row.name} ${column.label}`}
                          optional
                          value={value as number | null}
                          onChange={(next) => update(index, column.key, next)}
                        />
                      ) : (
                        <Input
                          aria-label={`${row.name} ${column.label}`}
                          value={value ?? ""}
                          onChange={(event) =>
                            update(
                              index,
                              column.key,
                              event.target.value || null,
                            )
                          }
                        />
                      )}
                    </td>
                  );
                })}
                <td>
                  <Button
                    variant="ghost"
                    onClick={() =>
                      edit((next) => {
                        next.variables[group] = next.variables[group].filter(
                          (name) => name !== row.name,
                        );
                        (next.io_definition[group] as IoRow[]).splice(index, 1);
                      })
                    }
                  >
                    Remove
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {!cfg.variables[group].length && (
          <p className="empty">Add variables to define this signal group.</p>
        )}
      </div>
      <section className="panel">
        <h2>Hardware tests</h2>
        <p className="muted">
          Tests use saved configuration. Save your draft first. A write test
          sends the entered values to hardware.
        </p>
        <div className="form-grid">
          {[
            ...cfg.variables.mvs.map((name) => `mv.${name}.write`),
            ...cfg.variables.cvs.map((name) => `cv.${name}.setpoint`),
          ].map((name) => (
            <Field key={name} label={name}>
              <NumberInput
                optional
                value={writeValues[name]}
                onChange={(value) =>
                  setWriteValues((previous) => {
                    const next = { ...previous };
                    if (value === null) delete next[name];
                    else next[name] = value;
                    return next;
                  })
                }
              />
            </Field>
          ))}
        </div>
        <div className="inline">
          <Button
            variant="outline"
            disabled={locked || dirty}
            onClick={() => void startJob({ kind: "io_read" }).catch(() => {})}
          >
            Run read test
          </Button>
          <Button
            variant="outline"
            disabled={locked || dirty || !Object.keys(writeValues).length}
            onClick={() =>
              void startJob({
                kind: "io_write",
                values: Object.fromEntries(
                  Object.entries(writeValues).map(([name, value]) => [
                    name,
                    { kind: "f64", value },
                  ]),
                ),
              }).catch(() => {})
            }
          >
            Write entered test values
          </Button>
        </div>
      </section>
      <InitialValues />
    </>
  );
}
function InitialValues() {
  const { config: cfg, edit } = useWorkspace();
  if (!cfg) return null;
  const points = [...cfg.io.inputs, ...cfg.io.outputs];
  return (
    <section className="panel">
      <h2>Typed initial inputs</h2>
      <p className="muted">
        Offline execution and fallback use these initial values. Boolean inputs
        remain booleans.
      </p>
      <div className="form-grid">
        {points.map((point) => (
          <Field key={point.name} label={point.name}>
            {point.signal_type === "bool" ? (
              <Select
                value={String(
                  cfg.logic.initial_values[point.name]?.value ?? true,
                )}
                onChange={(event) =>
                  edit((next) => {
                    next.logic.initial_values[point.name] = {
                      kind: "bool",
                      value: event.target.value === "true",
                    };
                  })
                }
              >
                <option value="true">Enabled / true</option>
                <option value="false">Disabled / false</option>
              </Select>
            ) : (
              <NumberInput
                value={Number(cfg.logic.initial_values[point.name]?.value ?? 0)}
                onChange={(value) =>
                  edit((next) => {
                    next.logic.initial_values[point.name] = {
                      kind: "f64",
                      value: value ?? 0,
                    };
                  })
                }
              />
            )}
          </Field>
        ))}
      </div>
    </section>
  );
}
function ModelEditor() {
  const { config: cfg, edit, locked } = useWorkspace();
  if (!cfg) return null;
  function setDimensions(count: number) {
    const n = Math.max(1, Math.min(100, Math.trunc(count)));
    const u = [...cfg!.variables.mvs, ...cfg!.variables.dvs];
    const y = cfg!.variables.cvs;
    edit((next) => {
      const old = next.model;
      next.model = {
        kind: "linear_state_space",
        state_count: n,
        u_order: u,
        y_order: y,
        a: resizeMatrix(old?.a ?? [], n, n),
        b: resizeMatrix(old?.b ?? [], n, u.length),
        c: resizeMatrix(old?.c ?? [], y.length, n),
        d: resizeMatrix(old?.d ?? [], y.length, u.length),
      };
    });
  }
  const model = cfg.model;
  return (
    <>
      <Notice>
        {cfg.logic.kind === "linear_mpc"
          ? "Linear MPC requires this model."
          : "The plant model is optional for this strategy. Simulation and observer replay use it."}
      </Notice>
      <div className="toolbar">
        <Field label="State dimensions">
          <NumberInput
            value={model?.state_count ?? 1}
            onChange={(value) => setDimensions(value ?? 1)}
          />
        </Field>
        <Button
          variant="outline"
          onClick={() => setDimensions(model?.state_count ?? 1)}
        >
          {model ? "Align to current signals" : "Create continuous model"}
        </Button>
        {model && (
          <Button
            variant="ghost"
            onClick={() =>
              edit((next) => {
                next.model = null;
              })
            }
          >
            Remove model
          </Button>
        )}
      </div>
      {model && (
        <>
          <section className="panel model-preview">
            <code>ẋ = A x + B u</code>
            <code>y = C x + D u</code>
            <p>
              Input order: {model.u_order.join(" → ") || "none"} · Output order:{" "}
              {model.y_order.join(" → ") || "none"}
            </p>
          </section>
          <div className="matrix-grid">
            {(["a", "b", "c", "d"] as const).map((key) => (
              <MatrixEditor
                key={key}
                name={key.toUpperCase()}
                matrix={model[key]}
                rows={key === "c" || key === "d" ? model.y_order : undefined}
                cols={key === "b" || key === "d" ? model.u_order : undefined}
                disabled={locked}
                onChange={(matrix) =>
                  edit((next) => {
                    if (next.model) next.model[key] = matrix;
                  })
                }
              />
            ))}
          </div>
        </>
      )}
    </>
  );
}
function Strategy() {
  const {
    config: cfg,
    edit,
    catalog,
    startJob,
    act,
    project,
    locked,
    dirty,
    refresh,
    setNotification,
  } = useWorkspace();
  const [source, setSource] = useState<{ source: string; revision: string }>();
  if (!cfg) return null;
  async function loadSource() {
    try {
      setSource(await request(`${projectPath(project)}/strategy/source`));
    } catch (error) {
      setNotification((error as Error).message);
    }
  }
  return (
    <>
      <section className="panel">
        <div className="form-grid">
          <Field label="Selected strategy">
            <Select
              value={cfg.logic.kind}
              onChange={(event) =>
                edit((next) => {
                  next.logic.kind = event.target
                    .value as ProjectConfig["logic"]["kind"];
                  next.logic.params = {};
                })
              }
            >
              <option value="p_controller">P controller</option>
              <option value="linear_mpc">Linear MPC</option>
              <option value="custom_rust">Custom Rust</option>
            </Select>
          </Field>
          <Field label="Controller cycle (ms)">
            <NumberInput
              value={cfg.logic.cycle_ms}
              onChange={(value) =>
                edit((next) => {
                  next.logic.cycle_ms = value ?? 1;
                })
              }
            />
          </Field>
        </div>
        <p className="muted">
          Rust validates strategy compatibility against the complete current I/O
          contract when saving and running.
        </p>
      </section>
      {cfg.logic.kind === "p_controller" && (
        <section className="panel">
          <h2>CV → MV pairings</h2>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>CV</th>
                  <th>MV</th>
                  <th>Gain kp</th>
                  <th>Bias</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {cfg.logic.p_controller.pairs.map((pair, index) => (
                  <tr key={index}>
                    <td>
                      <Select
                        value={pair.cv}
                        onChange={(event) =>
                          edit((next) => {
                            next.logic.p_controller.pairs[index].cv =
                              event.target.value;
                          })
                        }
                      >
                        {cfg.variables.cvs.map((name) => (
                          <option key={name}>{name}</option>
                        ))}
                      </Select>
                    </td>
                    <td>
                      <Select
                        value={pair.mv}
                        onChange={(event) =>
                          edit((next) => {
                            next.logic.p_controller.pairs[index].mv =
                              event.target.value;
                          })
                        }
                      >
                        {cfg.variables.mvs.map((name) => (
                          <option key={name}>{name}</option>
                        ))}
                      </Select>
                    </td>
                    <td>
                      <NumberInput
                        value={pair.kp}
                        onChange={(value) =>
                          edit((next) => {
                            next.logic.p_controller.pairs[index].kp =
                              value ?? 0;
                          })
                        }
                      />
                    </td>
                    <td>
                      <NumberInput
                        value={pair.bias}
                        onChange={(value) =>
                          edit((next) => {
                            next.logic.p_controller.pairs[index].bias =
                              value ?? 0;
                          })
                        }
                      />
                    </td>
                    <td>
                      <Button
                        variant="ghost"
                        onClick={() =>
                          edit((next) => {
                            next.logic.p_controller.pairs.splice(index, 1);
                          })
                        }
                      >
                        Remove
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Button
            variant="outline"
            disabled={!cfg.variables.cvs.length || !cfg.variables.mvs.length}
            onClick={() =>
              edit((next) => {
                next.logic.p_controller.pairs.push({
                  cv: cfg.variables.cvs[0],
                  mv: cfg.variables.mvs[0],
                  kp: 1,
                  bias: 0,
                });
              })
            }
          >
            Add pairing
          </Button>
        </section>
      )}
      {cfg.logic.kind === "linear_mpc" && (
        <Notice>
          Movement cost defaults to L2. L1 is a boolean manual choice. Tuning
          scores retain squared movement. External CV band edges are excluded
          from local tuning.
        </Notice>
      )}
      <section className="panel">
        <h2>Saved strategy parameters</h2>
        {dirty && (
          <p className="muted">
            Save strategy changes to obtain its authoritative parameter schema.
          </p>
        )}
        <div className="form-grid">
          {Object.entries(cfg.logic.params)
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([key, value]) => (
              <Field
                key={key}
                label={
                  key.endsWith(".use_l1")
                    ? `${key.replace(".use_l1", "")} · movement cost`
                    : key
                }
              >
                {typeof value === "boolean" ? (
                  <Select
                    value={String(value)}
                    onChange={(event) =>
                      edit((next) => {
                        next.logic.params[key] = event.target.value === "true";
                      })
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
                    value={value}
                    onChange={(next) =>
                      edit((cfg) => {
                        cfg.logic.params[key] = next ?? 0;
                      })
                    }
                  />
                )}
              </Field>
            ))}
        </div>
      </section>
      <section className="panel">
        <h2>Custom Rust strategy</h2>
        <p className="muted">
          Use your external editor or edit the project source below. Templates
          include history context and structured user-log helpers.
        </p>
        <p className="mono">
          {cfg.logic.custom_rust.source_path || "No template generated"}
        </p>
        <p>{cfg.logic.custom_rust.last_build_message}</p>
        <div className="inline">
          <Button
            variant="outline"
            disabled={locked || dirty}
            onClick={() =>
              void act(`${projectPath(project)}/strategy/template`).catch(
                () => {},
              )
            }
          >
            Generate template
          </Button>
          <Button
            variant="outline"
            disabled={locked || dirty}
            onClick={() =>
              void startJob({ kind: "strategy_build" }).catch(() => {})
            }
          >
            Build & check contract
          </Button>
          <Button variant="ghost" onClick={() => void loadSource()}>
            Load source
          </Button>
        </div>
        {source && (
          <>
            <textarea
              className="source-editor"
              spellCheck={false}
              aria-label="Custom Rust source"
              value={source.source}
              onChange={(event) =>
                setSource({ ...source, source: event.target.value })
              }
            />
            <div className="inline">
              <Button
                disabled={locked}
                onClick={() =>
                  void act(
                    `${projectPath(project)}/strategy/source`,
                    source,
                    "PUT",
                  )
                    .then(async (result) => {
                      setSource({
                        ...source,
                        revision: (result as { revision: string }).revision,
                      });
                      await refresh();
                    })
                    .catch(() => {})
                }
              >
                Save source
              </Button>
              <Button
                variant="outline"
                onClick={() => download("lib.rs", source.source)}
              >
                Download source
              </Button>
            </div>
          </>
        )}
      </section>
    </>
  );
}
function Settings() {
  const { config: cfg, edit, project, act, setNotification } = useWorkspace();
  if (!cfg) return null;
  return (
    <>
      <section className="panel">
        <h2>Fault progression</h2>
        <div className="form-grid">
          {(["alarm_timeout_ms", "off_timeout_ms"] as const).map((key) => (
            <Field key={key} label={key.replaceAll("_", " ")}>
              <NumberInput
                value={cfg.alarms[key]}
                onChange={(value) =>
                  edit((next) => {
                    next.alarms[key] = value ?? 0;
                  })
                }
              />
            </Field>
          ))}
        </div>
        <p className="muted">
          Runtime progresses through Running, Hold, Alarm and Off. Off writes no
          outputs.
        </p>
      </section>
      <section className="panel">
        <h2>Windows command metadata</h2>
        <div className="form-grid">
          {(["service_name", "display_name", "description"] as const).map(
            (key) => (
              <Field key={key} label={key.replaceAll("_", " ")}>
                <Input
                  value={cfg.deployment[key]}
                  onChange={(event) =>
                    edit((next) => {
                      next.deployment[key] = event.target.value;
                    })
                  }
                />
              </Field>
            ),
          )}
        </div>
      </section>
      <section className="panel">
        <h2>Project files</h2>
        <div className="inline">
          <Button asChild variant="outline">
            <a href={`${projectPath(project)}/config/download`}>
              Download saved TOML
            </a>
          </Button>
          <label className="upload-button">
            Import TOML into draft
            <input
              hidden
              type="file"
              accept=".toml"
              onChange={(event) => {
                const file = event.target.files?.[0];
                if (file)
                  void file
                    .text()
                    .then((toml) =>
                      act(`${projectPath(project)}/config/parse`, { toml }),
                    )
                    .then((result) =>
                      edit((next) => Object.assign(next, result)),
                    )
                    .catch(() => {});
              }}
            />
          </label>
        </div>
      </section>
    </>
  );
}
