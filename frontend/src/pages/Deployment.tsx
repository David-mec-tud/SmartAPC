import { useMutation, useQuery } from "@tanstack/react-query";
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
    jobs,
    workspace,
    strategyPending,
    save,
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
  const runningJob = jobs.data?.find((job) => job.phase === "running");
  const start = useMutation({
    mutationFn: () =>
      act("/api/execution/command", {
        action: "start",
        mode: "runtime",
        project_name: project,
      }),
  });
  // Rust owns saved configuration validity. Add only browser draft and backend
  // availability blockers here, and use the same list to explain the Start gate.
  const blockers: { message: string; page?: string }[] = [
    ...(readiness.data?.issues ?? []),
  ];
  if (dirty)
    blockers.push({
      message: "Save configuration in the bottom bar, or discard your draft.",
    });
  if (strategyPending || save.isPending)
    blockers.push({
      message: "Wait for configuration changes to finish saving.",
    });
  if (execution)
    blockers.push({
      message: `${execution.snapshot.project_name} is already executing in ${execution.mode} mode. Stop it in the top bar before starting a new runtime.`,
    });
  if (runningJob)
    blockers.push({
      message: `Wait for the ${runningJob.kind.replaceAll("_", " ")} job to finish.`,
    });
  else if (locked && !execution)
    blockers.push({
      message: "Wait for saved configuration to refresh after the job.",
    });
  if (readiness.isFetching || workspace.isPending || jobs.isPending)
    blockers.push({
      message: "Checking saved configuration and execution availability…",
    });
  if (readiness.error || workspace.error || jobs.error)
    blockers.push({
      message:
        "Restore the backend connection and refresh validation before starting.",
    });
  const canStart = !!readiness.data?.ready && blockers.length === 0;
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
        <h1>Validate &amp; run</h1>
        <p>
          Resolve the startup requirements, start in shadow mode, then verify
          live operation before enabling actuator writes.
        </p>
      </div>
      {dirty && (
        <Notice>
          Your draft has unsaved changes. Readiness and commands use saved
          configuration.
        </Notice>
      )}
      {readiness.error && <Notice error>{readiness.error.message}</Notice>}
      <section className="panel" aria-labelledby="startup-requirements">
        <div className="section-title">
          <h2 id="startup-requirements">
            <span className="step-number">1</span> Prepare for shadow mode
          </h2>
          <Badge tone={canStart ? "good" : "warning"}>
            {canStart
              ? "Ready for shadow mode"
              : active && execution.mode === "runtime"
                ? "Runtime active"
                : `${blockers.length} ${blockers.length === 1 ? "step" : "steps"} remaining`}
          </Badge>
        </div>
        <p className="muted">
          Required: saved configuration, a compatible strategy and I/O contract,
          and no other execution or running job for this project. The backend
          must be able to reach your OPC UA endpoint.
        </p>
        {blockers.length > 0 && (
          <div aria-label="Remaining startup steps">
            {blockers.map((blocker, index) => (
              <div key={index} className="readiness-row startup-blocker">
                <CircleAlert size={18} aria-hidden="true" />
                <span>{blocker.message}</span>
                {blocker.page && <Link to={blocker.page}>Resolve →</Link>}
              </div>
            ))}
          </div>
        )}
        {readiness.data?.ready && (
          <div className="readiness-row">
            <CheckCircle2 size={18} aria-hidden="true" />
            <span>
              Saved configuration and strategy/I/O compatibility checks passed.
            </span>
            <Badge tone="good">Passed</Badge>
          </div>
        )}
        {readiness.data && (
          <div className="readiness-row">
            <span>
              Plant model
              {readiness.data.model_requirement === "required"
                ? " · required by the selected strategy"
                : " · optional for the selected strategy"}
            </span>
            <Link to="/configuration/model">Review model →</Link>
          </div>
        )}
        <p className="muted">
          Configuration checks do not verify hardware reachability. You can test
          the <Link to="/configuration/io">saved I/O</Link> and{" "}
          <Link to="/configuration/communication">connection/watchdog</Link>{" "}
          before starting; successful test jobs are not a startup requirement.
        </p>
        <Button
          variant="outline"
          disabled={readiness.isFetching}
          onClick={() => {
            void readiness.refetch();
            void workspace.refetch();
            void jobs.refetch();
          }}
        >
          Refresh validation
        </Button>
      </section>
      {readiness.data && (
        <section className="panel" aria-labelledby="start-shadow-mode">
          <h2 id="start-shadow-mode">
            <span className="step-number">2</span> Start and verify shadow mode
          </h2>
          <p className="muted">
            Start runs the observer and controller suggestions with actuator
            writes disabled. Observer convergence and ControllerIsReady are
            checked after startup; they do not block starting in shadow mode.
            Watchdog exchanges and any configured status outputs remain active.
          </p>
          {config?.deployment.external_controller_enable?.trim() && (
            <Notice>
              ExternalControllerEnable is configured. A true value can enable
              actuator writes automatically once the controller is ready. Keep
              this external signal false to remain in shadow mode.
            </Notice>
          )}
          <div className="inline">
            <Button
              disabled={!canStart || start.isPending}
              onClick={() => start.mutate()}
            >
              {start.isPending ? "Starting runtime…" : "Start in shadow mode"}
            </Button>
            <Button asChild variant="outline">
              <a href={`${projectPath(project)}/config/download`}>
                <Download size={16} /> Saved project TOML
              </a>
            </Button>
          </div>
          {!canStart && !active && (
            <p className="muted">
              Complete the remaining startup steps above to start.
            </p>
          )}
          {start.error && <Notice error>{start.error.message}</Notice>}
          <details className="local-command">
            <summary>Start from a terminal instead</summary>
            <p className="muted">
              Use this command when the browser backend is not already running.
              It starts a backend and the saved project on a host with OPC UA
              access.
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
          </details>
        </section>
      )}
      <section className="panel" aria-labelledby="verify-runtime">
        <h2 id="verify-runtime">
          <span className="step-number">3</span> Verify before enabling control
        </h2>
        <p className="muted">
          Check connection health, advancing cycles, current inputs and
          controller suggestions in live diagnostics. Wait for observer
          convergence and a successful suggestion before enabling control.
          Without an external enable signal, use Turn controller on in the top
          bar when ready. ControllerHasControl stays false while actuator writes
          are disabled. Both status values remain visible here even without OPC
          UA addresses.
        </p>
        {active && execution.mode === "runtime" ? (
          <>
            <div className="readiness-row">
              <Badge tone="good">{execution.snapshot.state}</Badge>
              <Badge>
                {execution.snapshot.controller?.enabled
                  ? "Control enabled"
                  : "Shadow mode"}
              </Badge>
              <span>
                {execution.snapshot.metrics.total_cycles.toLocaleString()}{" "}
                controller cycles
              </span>
            </div>
            <div className="readiness-row">
              <span>OPC UA connection</span>
              <Badge
                tone={
                  execution.snapshot.connection_health === "healthy"
                    ? "good"
                    : "warning"
                }
              >
                {execution.snapshot.connection_health ?? "Unknown"}
              </Badge>
            </div>
            <div className="readiness-row">
              <span>Observer converged</span>
              <Badge
                tone={
                  execution.snapshot.controller?.observer_converged
                    ? "good"
                    : "warning"
                }
              >
                {String(
                  execution.snapshot.controller?.observer_converged ?? false,
                )}
              </Badge>
            </div>
            <div className="readiness-row">
              <span>ControllerIsReady</span>
              <Badge
                tone={
                  execution.snapshot.controller?.is_ready ? "good" : "warning"
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
                {String(execution.snapshot.controller?.has_control ?? false)}
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
              : "Not started. Start in shadow mode above to verify live hardware and controller readiness."}
          </Notice>
        )}
      </section>
      {config && (
        <section className="panel">
          <div className="section-title">
            <h2>Controller OPC UA handshake</h2>
            <Badge>Optional</Badge>
          </div>
          <p className="muted">
            All three Boolean Node IDs are optional. Leave ControllerIsReady and
            ControllerHasControl blank to keep their status in the UI without
            publishing it to OPC UA. ExternalControllerEnable is read only; when
            configured it owns on/off control. None is required to start.
          </p>
          <fieldset disabled={locked}>
            <div className="form-grid">
              {(
                [
                  [
                    "controller_is_ready",
                    "ControllerIsReady",
                    "Optional write · observer converged and a suggestion succeeded",
                  ],
                  [
                    "controller_has_control",
                    "ControllerHasControl",
                    "Optional write · enabled, ready, and actuator writes succeeded",
                  ],
                  [
                    "external_controller_enable",
                    "ExternalControllerEnable",
                    "Read only · optional external on/off",
                  ],
                ] as const
              ).map(([key, label, help]) => (
                <Field
                  key={key}
                  label={`${label} Node ID (optional)`}
                  hint={help}
                >
                  <Input
                    aria-label={`${label} Node ID`}
                    value={config.deployment[key] ?? ""}
                    placeholder="Optional · ns=2;s=..."
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
          <details className="panel">
            <summary>Optional engineering</summary>
            <p className="muted">
              These tasks do not block shadow-mode startup.
            </p>
            {readiness.data.optional.map((item) => (
              <div key={item} className="readiness-row">
                <span>{item}</span>
                <Badge>Optional</Badge>
              </div>
            ))}
          </details>
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
