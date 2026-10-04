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
