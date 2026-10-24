import { describe, expect, it } from "vitest";
import { groupParameters, parameterGroup } from "./parameters";
describe("online parameter navigation", () => {
  const params = {
    "mpc.cv.y1.q": 1,
    "mpc.mv.u1.use_l1": false,
    "mpc.kalman.cv.y1.r": 0.01,
    "io.cv.y1.optimization_mode_LL": -20,
    "custom.gain": 2,
  };
  it("separates control, observer and safety while preserving typed values", () => {
    expect(parameterGroup("mpc.cv.y1.q")).toBe("CV · y1");
    expect(parameterGroup("mpc.kalman.cv.y1.r")).toBe("Observer");
    expect(parameterGroup("io.cv.y1.optimization_mode_LL")).toBe(
      "Optimization safety",
    );
    expect(groupParameters(params, "movement")).toEqual([
      ["MV · u1", [["mpc.mv.u1.use_l1", false]]],
    ]);
  });
  it("searches keys, labels and groups without case sensitivity", () => {
    expect(
      groupParameters(params, "Y1").flatMap(([, entries]) => entries),
    ).toHaveLength(3);
    expect(groupParameters(params, "TRACKING")[0][1][0][0]).toBe("mpc.cv.y1.q");
    expect(groupParameters(params, "not present")).toEqual([]);
  });
});
