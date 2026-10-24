import type { LinearMpcConfig, MpcVariableObjective, Variables } from "../api";
import { NumberInput, Select } from "./ui";

/** Modes and priorities are engineering settings. Q/R weights still balance
 * objectives sharing a priority; Rust owns the staged optimization and bands. */
export function MpcObjectives({
  variables,
  objectives,
  onChange,
}: {
  variables: Variables;
  objectives: LinearMpcConfig;
  onChange: (
    group: "mvs" | "cvs",
    name: string,
    value: MpcVariableObjective,
  ) => void;
}) {
  return (
    <section className="panel">
      <h2>MV & CV optimisation objectives</h2>
      <p className="muted">
        Maximise seeks the high edge; minimise seeks the low edge. Minimum move
        favours holding MVs and the free CV prediction. MV low/high limits and
        CV setpoint-relative bands bound these objectives. Unreachable CV bands
        are recovered as far as possible before applying the objectives.
      </p>
      <p className="muted">
        Priority 0 is highest, then 1, 2, and so on. Lower priorities preserve
        higher-priority results within solver tolerance. Q/R weights balance
        objectives at the same priority. All minimum move at priority 0 retains
        the existing soft-band Q/R behaviour.
      </p>
      <div className="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Variable</th>
              <th>Optimisation mode</th>
              <th>Priority · 0 is highest</th>
            </tr>
          </thead>
          <tbody>
            {(["mvs", "cvs"] as const).flatMap((group) =>
              variables[group].map((name) => {
                const label = `${group === "mvs" ? "MV" : "CV"} ${name}`;
                const objective = objectives[group][name] ?? {
                  mode: "minimum_move" as const,
                  priority: 0,
                };
                return (
                  <tr key={`${group}.${name}`}>
                    <th scope="row">{label}</th>
                    <td>
                      <Select
                        aria-label={`${label} optimisation mode`}
                        value={objective.mode}
                        onChange={(event) =>
                          onChange(group, name, {
                            ...objective,
                            mode: event.target
                              .value as MpcVariableObjective["mode"],
                          })
                        }
                      >
                        <option value="maximize">Maximise</option>
                        <option value="minimize">Minimise</option>
                        <option value="minimum_move">Minimum move</option>
                      </Select>
                    </td>
                    <td>
                      <NumberInput
                        aria-label={`${label} priority`}
                        min={0}
                        step={1}
                        value={objective.priority}
                        onChange={(priority) =>
                          onChange(group, name, {
                            ...objective,
                            priority: priority ?? 0,
                          })
                        }
                      />
                    </td>
                  </tr>
                );
              }),
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}
