import { describe, it, expect } from "vitest";
import { futureTraces } from "./charts";
import type { Sample } from "./api";
const sample = (
  stamp: number,
  signal: string,
  value: number,
  direction = "user_log",
): Sample => ({
  timestamp_ms: stamp,
  signal,
  direction,
  value: { kind: "f64", value },
});
describe("MPC forecast alignment", () => {
  it("uses the prediction cycle, actual step indices, and its historical anchor", () => {
    const rows = [
      sample(1000, "cv.y1.process_value", 4, "input"),
      sample(1000, "future_pred_with_control.cv.y1.k01", 5),
      sample(1000, "future_pred_with_control.cv.y1.k03", 6),
      sample(1500, "cv.y1.process_value", 9, "input"),
    ];
    const trace = futureTraces(rows, "cv", "y1", "simulation", 100, 0.3)[0] as {
      x: number[];
      y: number[];
    };
    expect(trace.x).toEqual([1, 1.1, 1.3]);
    expect(trace.y).toEqual([4, 5, 6]);
  });
  it("supports legacy aliases, excludes old generations, and uses runtime dates", () => {
    const rows = [
      sample(1000, "mpc.pred.free.y1.k01", 2),
      sample(2000, "mpc.pred.free.y1.k01", 3),
      sample(2000, "mpc.pred.free.y1.k02", 4),
    ];
    const trace = futureTraces(rows, "cv", "y1", "runtime", 100, 0.1)[0] as {
      x: Date[];
      y: number[];
    };
    expect(trace.x.map((date) => date.getTime())).toEqual([2100]);
    expect(trace.y).toEqual([3]);
  });
  it("prefers MV output over readback and hides a disabled future window", () => {
    const rows = [
      sample(1000, "mv.u1.read", 4, "input"),
      sample(1000, "mv.u1.write", 5, "output"),
      sample(1000, "future_pred_with_control.mv.u1.k01", 6),
    ];
    expect(
      (
        futureTraces(rows, "mv", "u1", "simulation", 100, 1)[0] as {
          y: number[];
        }
      ).y,
    ).toEqual([5, 6]);
    expect(futureTraces(rows, "mv", "u1", "simulation", 100, 0)).toEqual([]);
  });
});

import {
  allocateTraceColors,
  colorTraces,
  makeTraces,
  canonicalSignal,
} from "./charts";
import type { Data } from "plotly.js";
describe("dashboard trace styling", () => {
  it("allocates distinct colors past the old palette and retains saved assignments", () => {
    const traces = makeTraces(
      [],
      Array.from({ length: 30 }, (_, index) => ({
        direction: "input",
        signal: `custom.signal${index}.read`,
        value_kind: "f64",
      })),
      "runtime",
    );
    const colors = allocateTraceColors(traces);
    expect(new Set(Object.values(colors)).size).toBe(30);
    const extended = allocateTraceColors(
      [
        ...traces,
        {
          name: "forecast",
          meta: { trace_identity: "forecast::cv.y1.controlled" },
        } as Data,
      ],
      colors,
    );
    for (const [key, value] of Object.entries(colors))
      expect(extended[key]).toBe(value);
  });
  it("shares color only within boundary pairs and dots boundaries by default", () => {
    const traces = makeTraces(
      [],
      [
        "cv.y1.process_value",
        "cv.y1.setpoint",
        "mpc.target.cv.y1.low",
        "mpc.target.cv.y1.high",
        "io.cv.y1.ll",
      ].map((signal) => ({ direction: "input", signal, value_kind: "f64" })),
      "simulation",
    );
    const styled = colorTraces(traces, allocateTraceColors(traces)) as {
      line: { color: string; dash: string };
      name: string;
    }[];
    expect(styled[2].line.color).toBe(styled[3].line.color);
    expect(new Set(styled.map((trace) => trace.line.color)).size).toBe(4);
    expect(styled[2].line.dash).toBe("dot");
    expect(styled[4].line.dash).toBe("dot");
    expect(styled[0].line.dash).toBe("solid");
    expect(styled[4].name).toContain("optimization_mode_LL");
    expect(canonicalSignal("mv.u1.ll")).toBe("mv.u1.ll");
  });
  it("reads old safety history through its canonical trace name", () => {
    const traces = makeTraces(
      [sample(1000, "io.cv.y1.ll", -20, "online_param")],
      [
        {
          direction: "online_param",
          signal: "io.cv.y1.optimization_mode_LL",
          value_kind: "f64",
        },
      ],
      "simulation",
    ) as { y: number[] }[];
    expect(traces[0].y).toEqual([-20]);
  });
});

import { cvBandTraces } from "./charts";
it("plots effective local/external bands and prefers authoritative Rust target logs", () => {
  const cv = {
    name: "y1",
    setpoint: "",
    process_value: "",
    band_low_value: -0.5,
    band_high_value: 0.5,
  };
  const rows = [
    sample(1000, "cv.y1.setpoint", 10, "input"),
    sample(1000, "mpc.cv.y1.band_low", -1, "online_param"),
  ];
  expect(
    (cvBandTraces(rows, cv, {}, "simulation")[0] as { y: number[] }).y,
  ).toEqual([9]);
  const external = [...rows, sample(1000, "cv.y1.band_low", -2, "input")];
  expect(
    (
      cvBandTraces(
        external,
        { ...cv, band_low_opc: "ns=2;s=band" },
        {},
        "simulation",
      )[0] as { y: number[] }
    ).y,
  ).toEqual([8]);
  expect(
    cvBandTraces(
      [
        ...rows,
        sample(1000, "mpc.target.cv.y1.low", 9.5),
        sample(1000, "mpc.target.cv.y1.high", 10.5),
      ],
      cv,
      {},
      "simulation",
    ),
  ).toEqual([]);
});
