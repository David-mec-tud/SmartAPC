import { useState, useEffect } from "react";
import { useQuery, useMutation } from "@tanstack/react-query";
import { Activity, Moon, Sun } from "lucide-react";
import type { Data } from "plotly.js";
import { request } from "../api";
import { Badge, Button, Field, Notice, NumberInput } from "../components/ui";
import { Plot } from "../components/Plot";
interface PlantSample {
  t_s: number;
  sample: { y: number; u: number; target: number; hl: number; ll: number };
}
interface PlantSnapshot {
  endpoint: string;
  namespace: number;
  config: { dt_ms: number; tau_s: number; gain: number; history: number };
  nodes: string[];
  history: PlantSample[];
  latest: PlantSample | null;
}
/** Standalone plant view has no editing project or execution actions. The OPC UA
 * plant owns its tick and continues when this page is closed. */
export function VirtualHardware() {
  const [dark, setDark] = useState(localStorage.getItem("theme") === "dark");
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("theme", dark ? "dark" : "light");
  }, [dark]);
  const snapshot = useQuery({
    queryKey: ["virtual-hardware"],
    queryFn: () => request<PlantSnapshot>("/api/virtual-hardware"),
    refetchInterval: 500,
  });
  const [manual, setManual] = useState({
    u_write: 0,
    target: 0,
    hl: 1,
    ll: -1,
  });
  const apply = useMutation({
    mutationFn: () => request("/api/virtual-hardware", manual, "POST"),
    onSuccess: () => void snapshot.refetch(),
  });
  const rows = snapshot.data?.history ?? [];
  const trace = (
    key: keyof PlantSample["sample"],
    title: string,
    color: string,
  ): Data => ({
    x: rows.map((row) => row.t_s),
    y: rows.map((row) => row.sample[key]),
    name: title,
    uid: key,
    type: "scatter",
    mode: "lines",
    line: { color },
  });
  return (
    <div className="plant-workspace">
      <header className="plant-header">
        <div className="inline">
          <Activity size={24} />
          <strong>
            SmartAPC <span className="muted">/ Virtual hardware</span>
          </strong>
        </div>
        <Button
          variant="ghost"
          aria-label={dark ? "Switch to light theme" : "Switch to dark theme"}
          onClick={() => setDark(!dark)}
        >
          {dark ? <Sun size={18} /> : <Moon size={18} />}
        </Button>
      </header>
      <main>
        <div className="page-title">
          <div className="eyebrow">Local OPC UA test plant</div>
          <h1>First-order process</h1>
          <p>
            Watch the real test plant, inspect its node addresses and queue
            manual values for its next tick.
          </p>
        </div>
        {snapshot.error && (
          <Notice error>
            {snapshot.error.message}. Start{" "}
            <code>smartapc virtual-hardware</code> and open its printed browser
            URL.
          </Notice>
        )}
        {snapshot.data && (
          <>
            <div className="toolbar">
              <code>{snapshot.data.endpoint}</code>
              <Badge tone={snapshot.isError ? "danger" : "good"}>
                {snapshot.isError ? "Feedback unavailable" : "Plant running"}
              </Badge>
            </div>
            <div className="metrics-grid">
              <div>
                <small>Process value</small>
                <strong>
                  {snapshot.data.latest?.sample.y.toFixed(4) ?? "—"}
                </strong>
              </div>
              <div>
                <small>Manipulated value</small>
                <strong>
                  {snapshot.data.latest?.sample.u.toFixed(4) ?? "—"}
                </strong>
              </div>
              <div>
                <small>Plant tick</small>
                <strong>{snapshot.data.config.dt_ms} ms</strong>
              </div>
              <div>
                <small>Time constant / gain</small>
                <strong>
                  {snapshot.data.config.tau_s} s / {snapshot.data.config.gain}
                </strong>
              </div>
            </div>
            <div className="chart-grid">
              <Plot
                title="CV · process, target and displayed boundaries"
                revision="plant-cv"
                data={[
                  trace("y", "y1_value", "#2c8a7a"),
                  trace("target", "y1_target", "#527ad4"),
                  trace("hl", "y1_HL", "#aa6458"),
                  trace("ll", "y1_LL", "#b17a34"),
                ]}
              />
              <Plot
                title="MV · u1_write / u1_read"
                revision="plant-mv"
                data={[trace("u", "u1", "#2c8a7a")]}
              />
            </div>
            <section className="panel">
              <h2>Manual plant inputs</h2>
              <p className="muted">
                Applying queues these values in the test plant. An active
                controller can subsequently replace the MV write.
              </p>
              <div className="form-grid">
                {(["u_write", "target", "hl", "ll"] as const).map((key) => (
                  <Field
                    key={key}
                    label={
                      {
                        u_write: "u1_write",
                        target: "y1_target",
                        hl: "y1_HL",
                        ll: "y1_LL",
                      }[key]
                    }
                  >
                    <NumberInput
                      value={manual[key]}
                      onChange={(value) =>
                        setManual((previous) => ({
                          ...previous,
                          [key]: value ?? 0,
                        }))
                      }
                    />
                  </Field>
                ))}
              </div>
              <Button disabled={apply.isPending} onClick={() => apply.mutate()}>
                Apply manual values
              </Button>
              {apply.error && <Notice error>{apply.error.message}</Notice>}
              {apply.isSuccess && (
                <p role="status">Values queued for the next plant tick.</p>
              )}
            </section>
            <details className="panel">
              <summary>OPC UA node addresses</summary>
              <p className="muted">
                Process value, readback and counter are read-only. Target,
                boundaries, MV write and watchdog mirror accept numeric writes.
              </p>
              <pre>{snapshot.data.nodes.join("\n")}</pre>
            </details>
          </>
        )}
      </main>
    </div>
  );
}
