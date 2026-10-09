import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { CheckCircle2, CircleAlert, Clipboard, Download } from "lucide-react";
import { type Readiness, download, projectPath, request } from "../api";
import { useWorkspace } from "../workspace";
import { Button, Badge, Notice, Field, Input } from "../components/ui";
import {
  HardwareTestResults,
  useHardwareTest,
} from "../components/HardwareTestResults";
export function Deployment() {
  const {
    project,
    execution,
    dirty,
    config,
    edit,
    locked,
    act,
    setNotification,
  } = useWorkspace();
  const test = useHardwareTest();
  const readiness = useQuery({
    queryKey: ["readiness", project],
    queryFn: () => request<Readiness>(`${projectPath(project)}/readiness`),
    enabled: !!project,
  });
  const commands = useQuery({
    queryKey: ["commands", project],
    queryFn: () =>
      request<Record<string, string>>(
        `/api/deployment/commands?project=${encodeURIComponent(project)}`,
      ),
    enabled: !!project,
  });
  const active = execution?.snapshot.project_name === project;
  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setNotification("Command copied.");
    } catch {
      setNotification(
        "Clipboard is unavailable. Download the commands instead.",
      );
    }
  }
  return (
    <>
      <div className="page-title">
        <div className="eyebrow">Deployment · Local / self-hosted</div>
        <h1>Validate, run, verify</h1>
        <p>
          Readiness follows the selected controller’s requirements. Engineering
          tasks remain optional.
        </p>
      </div>
      {dirty && (
        <Notice>
          Your draft has unsaved changes. Readiness and commands use saved
          configuration.
        </Notice>
      )}
      {readiness.error && <Notice error>{readiness.error.message}</Notice>}
      {config && (
        <section className="panel">
          <h2>
            <span className="step-number">1</span> Controller OPC UA handshake
          </h2>
          <p className="muted">
            Configure Boolean Node IDs. Status outputs are written by SmartAPC.
            ExternalControllerEnable is optional and read only; when configured
            it owns on/off control.
          </p>
          <fieldset disabled={locked}>
            <div className="form-grid">
              {(
                [
                  [
                    "controller_is_ready",
                    "ControllerIsReady",
                    "Write · observer converged and a suggestion succeeded",
                  ],
                  [
                    "controller_has_control",
                    "ControllerHasControl",
                    "Write · enabled, ready, and actuator writes succeeded",
                  ],
                  [
                    "external_controller_enable",
                    "ExternalControllerEnable",
                    "Read only · optional external on/off",
                  ],
                ] as const
              ).map(([key, label, help]) => (
                <Field key={key} label={`${label} Node ID`} hint={help}>
                  <Input
                    aria-label={`${label} Node ID`}
                    value={config.deployment[key] ?? ""}
                    placeholder="ns=2;s=..."
                    onChange={(event) =>
                      edit((next) => {
                        next.deployment[key] =
                          event.target.value.trim() || null;
                      })
                    }
                  />
                </Field>
              ))}
            </div>
          </fieldset>
          <div className="inline">
            <Button
              variant="outline"
              disabled={
                locked ||
                dirty ||
                test.isPending ||
                ![
                  config.deployment.controller_is_ready,
                  config.deployment.controller_has_control,
                  config.deployment.external_controller_enable,
                ].some((node) => node?.trim())
              }
              onClick={() => test.mutate({ kind: "controller_signals_test" })}
            >
              Test saved controller signals
            </Button>
          </div>
          <p className="muted">
            The test writes false to each status output and verifies readback.
            It reads ExternalControllerEnable without writing it. Run these
            tests with the controller stopped.
          </p>
          <HardwareTestResults
            kinds={["controller_signals_test"]}
            test={test}
          />
        </section>
      )}
      {readiness.data && (
        <>
          <section className="panel">
            <div className="section-title">
              <h2>
                <span className="step-number">2</span> Validate saved
                configuration
              </h2>
              <Badge tone={readiness.data.ready ? "good" : "danger"}>
                {readiness.data.ready
                  ? "Ready to start"
                  : "Configuration needs attention"}
              </Badge>
            </div>
            {readiness.data.issues.map((issue, index) => (
              <Link key={index} to={issue.page} className="readiness-row">
                <CircleAlert size={18} />
                <span>{issue.message}</span>
                <span>Edit →</span>
              </Link>
            ))}
            {readiness.data.ready && (
              <div className="readiness-row">
                <CheckCircle2 size={18} />
                <span>Strategy and I/O contract are compatible.</span>
              </div>
            )}
            <div className="readiness-row">
              <span>Plant model</span>
              <Badge>{readiness.data.model_requirement}</Badge>
            </div>
            {readiness.data.optional.map((item) => (
              <div key={item} className="readiness-row">
                <span>{item}</span>
                <Badge>Optional</Badge>
              </div>
            ))}
          </section>
          <section className="panel">
            <h2>
              <span className="step-number">3</span> Run the saved project
            </h2>
            <p className="muted">
              The backend must run on a host that can reach the configured OPC
              UA endpoint. Startup always enters shadow mode: suggestions and
              the observer run without actuator writes. Use Turn controller on
              after startup, or the optional ExternalControllerEnable signal.
            </p>
            <div className="command">
              <code>{readiness.data.local_command}</code>
              <Button
                variant="ghost"
                aria-label="Copy local run command"
                onClick={() => void copy(readiness.data!.local_command)}
              >
                <Clipboard size={16} />
              </Button>
            </div>
            <div className="inline">
              <Button
                disabled={!!execution || !readiness.data.ready || dirty}
                onClick={() =>
                  void act("/api/execution/command", {
                    action: "start",
                    mode: "runtime",
                    project_name: project,
                  }).catch(() => {})
                }
              >
                Start saved runtime
              </Button>
              <Button asChild variant="outline">
                <a href={`${projectPath(project)}/config/download`}>
                  <Download size={16} /> Saved project TOML
                </a>
              </Button>
            </div>
          </section>
          <section className="panel">
            <h2>
              <span className="step-number">4</span> Verify execution
            </h2>
            {active && execution.mode === "runtime" ? (
              <>
                <div className="readiness-row">
                  <Badge tone="good">{execution.snapshot.state}</Badge>
                  <span>
                    {execution.snapshot.metrics.total_cycles.toLocaleString()}{" "}
                    controller cycles
                  </span>
                </div>
                <div className="readiness-row">
                  <span>ControllerIsReady</span>
                  <Badge
                    tone={
                      execution.snapshot.controller?.is_ready
                        ? "good"
                        : "warning"
                    }
                  >
                    {String(execution.snapshot.controller?.is_ready ?? false)}
                  </Badge>
                </div>
                <div className="readiness-row">
                  <span>ControllerHasControl</span>
                  <Badge
                    tone={
                      execution.snapshot.controller?.has_control
                        ? "good"
                        : "neutral"
                    }
                  >
                    {String(
                      execution.snapshot.controller?.has_control ?? false,
                    )}
                  </Badge>
                </div>
                <p>{execution.snapshot.controller?.reason}</p>
                <p>
                  Watchdog: {JSON.stringify(execution.snapshot.watchdog_status)}
                </p>
                <Link to="/operation/diagnostics">Open live diagnostics →</Link>
              </>
            ) : (
              <Notice>
                {execution
                  ? `${execution.snapshot.project_name} is executing in ${execution.mode} mode.`
                  : "No runtime is executing. Hardware reachability has not been verified."}
              </Notice>
            )}
          </section>
          <details className="panel">
            <summary>Windows service command reference</summary>
            <p className="muted">{readiness.data.windows_service_support}</p>
            <p className="muted">
              These commands require review on Windows. The workspace generates
              commands; it does not install, remove or claim a running Windows
              service.
            </p>
            {Object.entries(commands.data ?? {}).map(([name, value]) => (
              <div key={name}>
                <h3>{name}</h3>
                <div className="command">
                  <code>{value}</code>
                  <Button
                    variant="ghost"
                    aria-label={`Copy ${name} command`}
                    onClick={() => void copy(value)}
                  >
                    <Clipboard size={16} />
                  </Button>
                </div>
              </div>
            ))}
            <Button
              variant="outline"
              onClick={() =>
                download(
                  `${project}-windows-commands.txt`,
                  Object.entries(commands.data ?? {})
                    .map(([name, value]) => `REM ${name}\n${value}`)
                    .join("\n\n"),
                )
              }
            >
              <Download size={16} /> Download commands
            </Button>
          </details>
        </>
      )}
    </>
  );
}
