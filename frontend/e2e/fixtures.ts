import { expect, type APIRequestContext } from "@playwright/test";
// Every run creates a separate temporary project on the test backend. No OPC UA
// test writes target any existing controller; engineering execution is simulated.
export async function seed(request: APIRequestContext, model = true) {
  const name = `journey_${Date.now()}_${Math.floor(Math.random() * 100000)}`;
  expect(
    (await request.post("/api/projects", { data: { name } })).ok(),
  ).toBeTruthy();
  const response = await request.get(`/api/projects/${name}/config`);
  const config = await response.json();
  Object.assign(config.variables, { mvs: ["u1"], cvs: ["y1"], dvs: [] });
  config.communication.endpoint_url = "mock://browser-journey";
  config.communication.cycle_ms = 100;
  config.io.cycle_ms = 100;
  config.logic.cycle_ms = 100;
  config.io_definition.mvs = [
    {
      name: "u1",
      mv_write: "ns=2;s=U1_WRITE",
      mv_read: "ns=2;s=U1_READ",
      ll: -10,
      hl: 10,
    },
  ];
  config.io_definition.cvs = [
    {
      name: "y1",
      setpoint: "ns=2;s=Y1_SP",
      process_value: "ns=2;s=Y1_PV",
      ll: -100,
      hl: 100,
    },
  ];
  config.logic.p_controller.pairs = [{ cv: "y1", mv: "u1", kp: 1, bias: 0 }];
  if (model)
    config.model = {
      kind: "linear_state_space",
      state_count: 1,
      u_order: ["u1"],
      y_order: ["y1"],
      a: [[-1]],
      b: [[1]],
      c: [[1]],
      d: [[0]],
    };
  config.simulation.cv_setpoints = { y1: 2 };
  expect(
    (
      await request.put(`/api/projects/${name}/config`, {
        data: config,
        headers: { "If-Match": response.headers().etag },
      })
    ).ok(),
  ).toBeTruthy();
  return name;
}

/** A failed assertion must not leave its disposable execution running and block
 * later journeys. The dedicated test backend contains only suite-owned projects. */
export async function stopTestExecution(request: APIRequestContext) {
  const response = await request.get("/api/execution/state");
  if (!response.ok()) return;
  const execution = await response.json();
  if (
    !execution ||
    !/^(journey_|hardware_)/.test(execution.snapshot.project_name)
  )
    return;
  const stopped = await request.post("/api/execution/command", {
    data: {
      action: "stop",
      mode: execution.mode,
      project_name: execution.snapshot.project_name,
      execution_id: execution.snapshot.execution_id,
    },
  });
  // A clicked UI Stop may finish between this snapshot and cleanup command.
  if (
    !stopped.ok() &&
    !(await (await request.get("/api/execution/state")).json())
  )
    return;
  expect(stopped.ok()).toBeTruthy();
}
