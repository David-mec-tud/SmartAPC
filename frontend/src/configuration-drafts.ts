import type { ProjectConfig } from "./api";

/** Rename references in a draft together with its I/O row. Rust still validates
 * the resulting contract on Save. Compiled custom metadata is deliberately kept:
 * a renamed I/O contract must fail compatibility until its source is rebuilt. */
export function renameVariable(
  config: ProjectConfig,
  group: "mvs" | "cvs" | "dvs",
  index: number,
  nextName: string,
) {
  const previous = config.io_definition[group][index].name;
  const kind = group.slice(0, -1);
  config.io_definition[group][index].name = nextName;
  config.variables[group] = config.variables[group].map((name) =>
    name === previous ? nextName : name,
  );
  const signal = (key: string) => {
    const parts = key.split(".");
    for (let position = 0; position + 1 < parts.length; position++)
      if (parts[position] === kind && parts[position + 1] === previous)
        parts[position + 1] = nextName;
    if (parts[0] === "p") {
      const position = kind === "cv" ? 1 : kind === "mv" ? 2 : -1;
      if (position >= 0 && parts[position] === previous)
        parts[position] = nextName;
    }
    return parts.join(".");
  };
  const mapNames = (map: Record<string, number>) => {
    if (previous in map) {
      map[nextName] = map[previous];
      delete map[previous];
    }
  };
  for (const pair of config.logic.p_controller.pairs) {
    if (kind === "cv" && pair.cv === previous) pair.cv = nextName;
    if (kind === "mv" && pair.mv === previous) pair.mv = nextName;
  }
  if (config.model) {
    config.model.u_order = config.model.u_order.map((name) =>
      kind !== "cv" && name === previous ? nextName : name,
    );
    config.model.y_order = config.model.y_order.map((name) =>
      kind === "cv" && name === previous ? nextName : name,
    );
  }
  for (const section of [config.simulation, config.optimization])
    for (const [key, value] of Object.entries(section))
      if (
        key.startsWith(`${kind}_`) &&
        value &&
        typeof value === "object" &&
        !Array.isArray(value)
      )
        mapNames(value as Record<string, number>);
  config.logic.initial_values = Object.fromEntries(
    Object.entries(config.logic.initial_values).map(([key, value]) => [
      signal(key),
      value,
    ]),
  );
  config.logic.params = Object.fromEntries(
    Object.entries(config.logic.params).map(([key, value]) => [
      key.startsWith("io.") || config.logic.kind !== "custom_rust"
        ? signal(key)
        : key,
      value,
    ]),
  );
  for (const row of [
    ...config.optimization.parameters,
    ...config.online_optimization.parameters,
  ])
    row.key = signal(row.key);
  for (const row of config.online_optimization.cvs)
    if (kind === "cv" && row.name === previous) row.name = nextName;
  for (const row of config.online_optimization.delta_mvs)
    if (kind === "mv" && row.name === previous) row.name = nextName;
  const order = `${kind}_order` as "mv_order" | "cv_order" | "dv_order";
  config.plots[order] = config.plots[order].map((name) =>
    name === previous ? nextName : name,
  );
  for (const plot of config.plots.custom_plots)
    for (const trace of plot.traces) trace.signal = signal(trace.signal);
}
