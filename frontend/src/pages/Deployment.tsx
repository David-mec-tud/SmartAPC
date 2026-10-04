import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { CheckCircle2, CircleAlert, Clipboard, Download } from "lucide-react";
import { type Readiness, download, projectPath, request } from "../api";
import { useWorkspace } from "../workspace";
import { Button, Badge, Notice } from "../components/ui";
export function Deployment() {
  const { project, execution, dirty, act, setNotification } = useWorkspace();
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
      {readiness.data && (
        <>
          <section className="panel">
            <div className="section-title">
              <h2>
                <span className="step-number">1</span> Validate saved
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
              <span className="step-number">2</span> Run the saved project
            </h2>
            <p className="muted">
              The backend must run on a host that can reach the configured OPC
              UA endpoint. Starting explicitly connects and enables controller
              output writes.
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
              <span className="step-number">3</span> Verify execution
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
