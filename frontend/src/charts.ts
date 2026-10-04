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
      uid: traceId(`${series.direction}::${series.signal}`),
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
        uid: traceId(`${kind}.${name}.${prefix}`),
        line: { dash: "dot", color },
      } as Data,
    ];
  });
}
