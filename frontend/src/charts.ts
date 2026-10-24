import type { Data } from "plotly.js";
import type { Mode, Sample, Series } from "./api";

/** Plotly uses trace IDs in CSS selectors during cleanup. Encode signal names
 * instead of passing dots/colons that can break panel changes and unmounts. */
function traceId(identity: string): string {
  return (
    "trace" +
    Array.from(identity, (character) =>
      character.codePointAt(0)!.toString(16),
    ).join("-")
  );
}

export function makeTraces(
  samples: Sample[],
  selected: Series[],
  mode: Mode,
): Data[] {
  const traces: Data[] = selected.map((series) => {
    const identity = `${series.direction}::${canonicalSignal(series.signal)}`;
    const points = samples.filter(
      (row) =>
        row.direction === series.direction &&
        canonicalSignal(row.signal) === canonicalSignal(series.signal),
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
      name: /^mpc\.target\.cv\./.test(series.signal)
        ? series.signal.replace(
            /^mpc\.target\.cv\.(.*)\.(low|high)$/,
            "$1 · Band $2",
          )
        : `${canonicalSignal(series.signal)} · ${series.direction}`,
      meta: { trace_identity: identity },
      uid: traceId(`${series.direction}::${series.signal}`),
      type: "scatter",
      mode: "lines",
      line: {
        shape: series.value_kind === "bool" ? "hv" : "linear",
        dash: isBoundary(series.signal) ? "dot" : "solid",
      },
    };
  });
  return colorTraces(traces, allocateTraceColors(traces));
}
/** Preserve old history on disk while presenting the current safety terminology. */
export function canonicalSignal(signal: string): string {
  return signal
    .replace(/^(io\.cv\..*)\.ll$/, "$1.optimization_mode_LL")
    .replace(/^(io\.cv\..*)\.hl$/, "$1.optimization_mode_HL");
}
function isBoundary(signal: string) {
  return /\.(ll|hl|low|high|band_low|band_high|optimization_mode_LL|optimization_mode_HL)$/.test(
    signal,
  );
}
function colorIdentity(identity: string) {
  // The two edges of one boundary family may share a color, but not another signal.
  return identity
    .replace(/\.(ll|hl)$/, ".limit_pair")
    .replace(/\.(low|high)$/, ".edge_pair")
    .replace(/\.band_(low|high)$/, ".band_pair")
    .replace(/\.optimization_mode_(LL|HL)$/, ".optimization_pair");
}
export function traceIdentity(trace: Data): string {
  const scatter = trace as {
    meta?: { trace_identity?: string };
    uid?: string;
    name?: string;
  };
  return scatter.meta?.trace_identity ?? String(scatter.uid ?? scatter.name);
}
const palette = [
  "#268574",
  "#527ad4",
  "#b17a34",
  "#b65b99",
  "#718f37",
  "#b4574a",
  "#805ec2",
  "#218bb0",
  "#c26b28",
  "#717580",
  "#b13e66",
  "#168867",
];
/** Allocate across all dashboard panels, retaining saved assignments as panels change.
 * Generated hues extend the palette instead of wrapping and reusing a trace color. */
export function allocateTraceColors(
  traces: Data[],
  saved: Record<string, string> = {},
) {
  const colors = { ...saved };
  const used = new Set(Object.values(colors));
  for (const key of [
    ...new Set(traces.map((trace) => colorIdentity(traceIdentity(trace)))),
  ].sort()) {
    if (colors[key]) continue;
    let candidate = palette.find((color) => !used.has(color));
    let index = 0;
    while (!candidate || used.has(candidate)) {
      candidate = `hsl(${((index++ * 137.508) % 360).toFixed(3)}, 62%, 43%)`;
    }
    colors[key] = candidate;
    used.add(candidate);
  }
  return colors;
}
export function colorTraces(
  traces: Data[],
  colors: Record<string, string>,
): Data[] {
  return traces.map(
    (trace) =>
      ({
        ...trace,
        line: {
          ...("line" in trace ? trace.line : {}),
          color: colors[colorIdentity(traceIdentity(trace))],
        },
      }) as Data,
  );
}

/** Use one prediction generation, its own cycle timestamp, and actual step
 * indices. Legacy custom controllers can emit the original MPC aliases. */
export function futureTraces(
  samples: Sample[],
  kind: "mv" | "cv",
  name: string,
  mode: Mode,
  cycleMs: number,
  futureSec: number,
): Data[] {
  if (!futureSec || !samples.length || cycleMs <= 0) return [];
  const predictions =
    kind === "cv"
      ? [
          {
            prefix: "future_pred_without_control.cv",
            alias: "mpc.pred.free",
            title: "Future · no control",
            color: "#b17a34",
          },
          {
            prefix: "future_pred_with_control.cv",
            alias: "mpc.pred.mpc",
            title: "Future · with control",
            color: "#2c8a7a",
          },
        ]
      : [
          {
            prefix: "future_pred_with_control.mv",
            alias: "",
            title: "Future · with control",
            color: "#2c8a7a",
          },
        ];
  return predictions.flatMap(({ prefix, alias, title, color }) => {
    const matching = (key: string) =>
      samples.filter(
        (row) =>
          row.direction === "user_log" &&
          row.value.kind === "f64" &&
          row.signal.startsWith(`${key}.${name}.k`),
      );
    let matches = matching(prefix);
    if (!matches.length && alias) matches = matching(alias);
    if (!matches.length) return [];
    const stamp = Math.max(...matches.map((row) => row.timestamp_ms));
    const horizon = matches
      .filter((row) => row.timestamp_ms === stamp)
      .map((row) => ({
        step: Number(row.signal.split(".k").at(-1)),
        value: Number(row.value.value),
      }))
      .filter(
        (row) =>
          Number.isInteger(row.step) &&
          row.step > 0 &&
          row.step * cycleMs <= futureSec * 1000,
      )
      .sort((a, b) => a.step - b.step);
    if (!horizon.length) return [];
    const anchorKeys =
      kind === "cv"
        ? [{ direction: "input", signal: `cv.${name}.process_value` }]
        : [
            { direction: "output", signal: `mv.${name}.write` },
            { direction: "input", signal: `mv.${name}.read` },
          ];
    let anchor: Sample | undefined;
    for (const key of anchorKeys) {
      anchor = samples
        .filter(
          (row) =>
            row.direction === key.direction &&
            row.signal === key.signal &&
            row.timestamp_ms <= stamp &&
            row.value.kind === "f64",
        )
        .reduce<Sample | undefined>(
          (latest, row) =>
            !latest || row.timestamp_ms > latest.timestamp_ms ? row : latest,
          undefined,
        );
      if (anchor) break;
    }
    const timestamp = (ms: number) =>
      mode === "simulation" ? ms / 1000 : new Date(ms);
    return [
      {
        x: [
          ...(anchor ? [timestamp(stamp)] : []),
          ...horizon.map((row) => timestamp(stamp + row.step * cycleMs)),
        ],
        y: [
          ...(anchor ? [Number(anchor.value.value)] : []),
          ...horizon.map((row) => row.value),
        ],
        type: "scatter",
        mode: "lines",
        name: title,
        meta: { trace_identity: `forecast::${kind}.${name}.${prefix}` },
        uid: traceId(`${kind}.${name}.${prefix}`),
        line: { dash: "dot", color },
      } as Data,
    ];
  });
}

/** Controllers that do not emit MPC diagnostics still get setpoint-relative bands.
 * Prefer Rust's resolved target logs whenever present, especially for external edges. */
export function cvBandTraces(
  samples: Sample[],
  cv: import("./api").Cv,
  params: import("./api").Parameters,
  mode: Mode,
): Data[] {
  const missing = (["low", "high"] as const).filter(
    (edge) =>
      !samples.some(
        (sample) =>
          sample.direction === "user_log" &&
          sample.signal === `mpc.target.cv.${cv.name}.${edge}`,
      ),
  );
  if (!missing.length) return [];
  let setpoint: number | undefined;
  const legacy = !cv.deadband_opc ? Math.abs(cv.deadband_value ?? 0) : 0;
  const offsets = {
    low: Number(
      params[`mpc.cv.${cv.name}.band_low`] ?? cv.band_low_value ?? -legacy,
    ),
    high: Number(
      params[`mpc.cv.${cv.name}.band_high`] ?? cv.band_high_value ?? legacy,
    ),
  };
  const groups = new Map<number, Sample[]>();
  for (const row of samples) {
    if (
      row.signal === `cv.${cv.name}.setpoint` ||
      (/\.(band_low|band_high)$/.test(row.signal) &&
        row.signal.includes(`.${cv.name}.`))
    )
      groups.set(row.timestamp_ms, [
        ...(groups.get(row.timestamp_ms) ?? []),
        row,
      ]);
  }
  const synthetic: Sample[] = [];
  for (const [timestamp_ms, rows] of [...groups.entries()].sort(
    ([a], [b]) => a - b,
  )) {
    for (const row of rows) {
      if (row.value.kind !== "f64") continue;
      if (row.signal === `cv.${cv.name}.setpoint`) setpoint = row.value.value;
      for (const edge of ["low", "high"] as const) {
        const external = edge === "low" ? cv.band_low_opc : cv.band_high_opc;
        if (
          external
            ? row.direction === "input" &&
              row.signal === `cv.${cv.name}.band_${edge}`
            : row.direction === "online_param" &&
              row.signal === `mpc.cv.${cv.name}.band_${edge}`
        )
          offsets[edge] = row.value.value;
      }
    }
    if (setpoint === undefined) continue;
    for (const edge of missing)
      synthetic.push({
        timestamp_ms,
        direction: "user_log",
        signal: `mpc.target.cv.${cv.name}.${edge}`,
        value: {
          kind: "f64",
          value:
            setpoint +
            (edge === "low"
              ? Math.min(offsets.low, offsets.high)
              : Math.max(offsets.low, offsets.high)),
        },
      });
  }
  return makeTraces(
    synthetic,
    missing.map((edge) => ({
      direction: "user_log",
      signal: `mpc.target.cv.${cv.name}.${edge}`,
      value_kind: "f64",
    })),
    mode,
  );
}
