import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import {
  BrowserRouter,
  Link,
  NavLink,
  Route,
  Routes,
  useLocation,
} from "react-router-dom";
import {
  Activity,
  Cable,
  ChartNoAxesCombined,
  CircleGauge,
  Code2,
  FlaskConical,
  Layers3,
  Menu,
  Moon,
  Play,
  Plus,
  Rocket,
  Save,
  Settings2,
  Square,
  Sun,
  Terminal,
  X,
} from "lucide-react";
import { type Readiness, projectPath, request } from "./api";
import { WorkspaceProvider, useWorkspace } from "./workspace";
import { Configuration } from "./pages/Configuration";
import { VirtualHardware } from "./pages/VirtualHardware";
import { Engineering } from "./pages/Engineering";
import { Operation, Trends } from "./pages/Operation";
import { Deployment } from "./pages/Deployment";
import { Button, Badge, Input, Notice, Select } from "./components/ui";
import "./styles.css";

const groups = [
  {
    title: "Workspace",
    items: [{ to: "/", label: "Overview", icon: CircleGauge }],
  },
  {
    title: "Project configuration",
    items: [
      {
        to: "/configuration/communication",
        label: "Connection & timing",
        icon: Cable,
      },
      { to: "/configuration/io", label: "Variables & I/O", icon: Layers3 },
      {
        to: "/configuration/model",
        label: "Plant model",
        icon: ChartNoAxesCombined,
      },
      { to: "/configuration/strategy", label: "Control strategy", icon: Code2 },
      {
        to: "/configuration/settings",
        label: "Project settings",
        icon: Settings2,
      },
    ],
  },
  {
    title: "Engineering",
    items: [
      {
        to: "/engineering/simulation",
        label: "Simulation",
        icon: FlaskConical,
      },
      { to: "/engineering/tests", label: "Control tests", icon: Activity },
      {
        to: "/engineering/kalman",
        label: "Kalman replay",
        icon: ChartNoAxesCombined,
      },
      {
        to: "/engineering/optimization",
        label: "Offline optimization",
        icon: Settings2,
      },
    ],
  },
  {
    title: "Operation",
    items: [
      { to: "/operation/trends", label: "Trends", icon: ChartNoAxesCombined },
      {
        to: "/operation/parameters",
        label: "Online parameters",
        icon: Settings2,
      },
      {
        to: "/operation/optimization",
        label: "Online optimization",
        icon: Activity,
      },
      { to: "/operation/logs", label: "Logs & events", icon: Terminal },
      { to: "/operation/diagnostics", label: "Diagnostics", icon: CircleGauge },
    ],
  },
  {
    title: "Deployment",
    items: [{ to: "/deployment", label: "Validate & run", icon: Rocket }],
  },
];
function Shell() {
  const state = useWorkspace();
  const location = useLocation();
  const {
    project,
    setProject,
    projects,
    workspace,
    execution,
    config,
    notification,
    setNotification,
    dirty,
    save,
    discard,
    locked,
    act,
  } = state;
  const [dark, setDark] = useState(localStorage.getItem("theme") === "dark");
  const [mobile, setMobile] = useState(false);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [pending, setPending] = useState(false);
  const [, setTick] = useState(0);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("theme", dark ? "dark" : "light");
  }, [dark]);
  useEffect(() => {
    const interval = setInterval(() => setTick((value) => value + 1), 1000);
    return () => clearInterval(interval);
  }, []);
  useEffect(() => setMobile(false), [location.pathname]);
  const age = workspace.dataUpdatedAt
    ? Math.round((Date.now() - workspace.dataUpdatedAt) / 1000)
    : 0;
  const stale = workspace.isError || (!!workspace.dataUpdatedAt && age > 10);
  return (
    <div className="workspace-shell">
      <aside className={`sidebar ${mobile ? "sidebar-open" : ""}`}>
        <Link to="/" className="brand">
          <span className="brand-mark">
            <Activity size={23} />
          </span>
          <span>
            SmartAPC<small>Control workspace</small>
          </span>
        </Link>
        <div className="project-picker">
          <label htmlFor="project">Editing project</label>
          <div className="inline">
            <Select
              id="project"
              value={project}
              onChange={(event) => setProject(event.target.value)}
            >
              {!projects.data?.length && (
                <option value="">Choose a project</option>
              )}
              {projects.data?.map((name) => (
                <option key={name}>{name}</option>
              ))}
            </Select>
            <Button
              variant="ghost"
              size="small"
              aria-label="Create project"
              onClick={() => setCreating(!creating)}
            >
              <Plus size={18} />
            </Button>
          </div>
          {creating && (
            <form
              className="new-project"
              onSubmit={(event) => {
                event.preventDefault();
                setPending(true);
                void act("/api/projects", { name })
                  .then(() => {
                    setProject(name);
                    setCreating(false);
                    setName("");
                  })
                  .catch(() => {})
                  .finally(() => setPending(false));
              }}
            >
              <Input
                aria-label="New project name"
                autoFocus
                value={name}
                placeholder="Project name"
                pattern="[A-Za-z0-9_-]+"
                onChange={(event) => setName(event.target.value)}
              />
              <Button disabled={pending || !name.trim()} size="small">
                Create project
              </Button>
            </form>
          )}
        </div>
        <nav aria-label="Main navigation">
          {groups.map((group) => (
            <div className="nav-group" key={group.title}>
              <div className="nav-heading">{group.title}</div>
              {group.items.map((item) => (
                <NavLink key={item.to} to={item.to} end={item.to === "/"}>
                  <item.icon size={16} />
                  <span>{item.label}</span>
                </NavLink>
              ))}
            </div>
          ))}
        </nav>
        <footer>
          <span>Rust backend · Local assets</span>
          <Button
            variant="ghost"
            size="small"
            aria-label={dark ? "Switch to light theme" : "Switch to dark theme"}
            onClick={() => setDark(!dark)}
          >
            {dark ? <Sun size={17} /> : <Moon size={17} />}
          </Button>
        </footer>
      </aside>
      <div className="main-column">
        <header className="execution-bar">
          <Button
            className="mobile-menu"
            variant="ghost"
            aria-label="Toggle navigation"
            onClick={() => setMobile(!mobile)}
          >
            <Menu size={20} />
          </Button>
          <div className="execution-identity">
            <Badge
              tone={
                stale || execution?.snapshot.state === "alarm"
                  ? "danger"
                  : execution?.snapshot.state === "hold"
                    ? "warning"
                    : execution?.snapshot.state === "running"
                      ? "good"
                      : "neutral"
              }
            >
              {stale
                ? "Connection stale"
                : (execution?.snapshot.state ?? "Idle")}
            </Badge>
            <strong>
              {execution ? execution.snapshot.project_name : "Backend ready"}
            </strong>
            <span>{execution?.mode ?? "No active execution"}</span>
            {execution?.mode === "runtime" && (
              <Badge
                tone={
                  execution.snapshot.connection_health === "healthy"
                    ? "good"
                    : "danger"
                }
              >
                OPC UA · {execution.snapshot.connection_health ?? "unknown"}
              </Badge>
            )}
            {execution?.mode === "runtime" && (
              <span>
                Watchdog ·{" "}
                {typeof execution.snapshot.watchdog_status === "string"
                  ? execution.snapshot.watchdog_status
                  : "fault"}
              </span>
            )}
            {execution?.snapshot.project_name !== project && execution && (
              <Badge>Editing {project}</Badge>
            )}
          </div>
          <div className="execution-actions">
            <span className="data-age">
              {age < 60 ? `${age}s ago` : "unavailable"}
            </span>
            {execution ? (
              <Button
                variant="destructive"
                size="small"
                disabled={pending || stale}
                onClick={() => {
                  setPending(true);
                  void act("/api/execution/command", {
                    action: "stop",
                    project_name: execution.snapshot.project_name,
                    mode: execution.mode,
                    execution_id: execution.snapshot.execution_id,
                  })
                    .catch(() => {})
                    .finally(() => setPending(false));
                }}
              >
                <Square size={13} /> Stop {execution.mode}
              </Button>
            ) : (
              <Button asChild variant="outline" size="small">
                <Link to="/deployment">
                  <Play size={13} /> Run project
                </Link>
              </Button>
            )}
          </div>
        </header>
        <main>
          {notification && (
            <div className="notification" role="status">
              <span>{notification}</span>
              <Button
                variant="ghost"
                size="small"
                aria-label="Dismiss notification"
                onClick={() => setNotification("")}
              >
                <X size={16} />
              </Button>
            </div>
          )}
          {stale && (
            <Notice error>
              The browser cannot confirm fresh backend state. Reconnection will
              resynchronize execution; the controller continues independently.
            </Notice>
          )}
          {projects.error && <Notice error>{projects.error.message}</Notice>}
          {state.saved.error && (
            <Notice error>{state.saved.error.message}</Notice>
          )}
          {!project ? (
            <div className="welcome">
              <div className="eyebrow">Welcome to SmartAPC</div>
              <h1>
                Build a controller.
                <br />
                See it work.
              </h1>
              <p>
                Create a project to configure signals, engineer control and run
                locally.
              </p>
              <Button onClick={() => setCreating(true)}>
                <Plus size={16} /> Create project
              </Button>
            </div>
          ) : !config ? (
            <p className="empty">Loading saved project…</p>
          ) : (
            <Routes>
              <Route path="/" element={<Overview />} />
              <Route
                path="/configuration/:section"
                element={<Configuration />}
              />
              <Route path="/engineering/:section" element={<Engineering />} />
              <Route path="/operation/:section" element={<Operation />} />
              <Route path="/deployment" element={<Deployment />} />
              <Route
                path="*"
                element={
                  <Notice>
                    Page not found. <Link to="/">Return to overview</Link>
                  </Notice>
                }
              />
            </Routes>
          )}
        </main>
        {project && (
          <div className="save-bar">
            <span>
              {dirty ? (
                <>
                  <span className="unsaved-dot" /> Unsaved draft · {project}
                </>
              ) : (
                <>Saved configuration · {project}</>
              )}
            </span>
            <div className="inline">
              <Button
                variant="ghost"
                size="small"
                disabled={!dirty || save.isPending}
                onClick={discard}
              >
                Discard draft
              </Button>
              <Button
                size="small"
                disabled={!dirty || locked || save.isPending}
                onClick={() => save.mutate()}
              >
                <Save size={14} />
                {save.isPending ? "Saving…" : "Save configuration"}
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
function Overview() {
  const { project, config, execution, jobs } = useWorkspace();
  const readiness = useQuery({
    queryKey: ["readiness", project],
    queryFn: () => request<Readiness>(`${projectPath(project)}/readiness`),
    enabled: !!project,
  });
  const active = execution?.snapshot.project_name === project;
  return (
    <>
      <div className="page-title overview-title">
        <div>
          <div className="eyebrow">Engineering workspace · {project}</div>
          <h1>Control, with clarity.</h1>
          <p>
            Configure the plant, evaluate control and operate from one local
            workspace.
          </p>
        </div>
        <Badge tone={readiness.data?.ready ? "good" : "neutral"}>
          {readiness.data?.ready
            ? "Saved project ready"
            : "Configuration in progress"}
        </Badge>
      </div>
      <div className="metrics-grid overview-metrics">
        <div>
          <small>Selected strategy</small>
          <strong>{config?.logic.kind.replaceAll("_", " ")}</strong>
          <Link to="/configuration/strategy">Configure strategy →</Link>
        </div>
        <div>
          <small>Signal catalog</small>
          <strong>
            {config?.variables.mvs.length} MV · {config?.variables.cvs.length}{" "}
            CV · {config?.variables.dvs.length} DV
          </strong>
          <Link to="/configuration/io">Edit I/O contract →</Link>
        </div>
        <div>
          <small>Execution</small>
          <strong>
            {active
              ? `${execution.mode} · ${execution.snapshot.state}`
              : "Idle for this project"}
          </strong>
          <Link to="/deployment">Validate & run →</Link>
        </div>
      </div>
      <section className="panel">
        <div className="section-title">
          <h2>Process trends</h2>
          <Link to="/operation/trends">Open trend workspace →</Link>
        </div>
        <Trends compact />
      </section>
      <div className="editor-grid">
        <section className="panel">
          <h2>Timing ownership</h2>
          <div className="readiness-row">
            <span>Communication / watchdog</span>
            <strong>{config?.communication.cycle_ms} ms</strong>
          </div>
          <div className="readiness-row">
            <span>I/O exchanges</span>
            <strong>{config?.io.cycle_ms} ms</strong>
          </div>
          <div className="readiness-row">
            <span>Controller / simulation</span>
            <strong>{config?.logic.cycle_ms} ms</strong>
          </div>
        </section>
        <section className="panel">
          <h2>Recent engineering work</h2>
          {jobs.data?.slice(0, 4).map((job) => (
            <div className="readiness-row" key={job.id}>
              <span>{job.kind.replaceAll("_", " ")}</span>
              <Badge tone={job.phase === "failed" ? "danger" : "neutral"}>
                {job.phase}
              </Badge>
            </div>
          ))}
          {!jobs.data?.length && (
            <p className="muted">
              Evaluate a simulation, run control tests or replay an observer.
              Engineering work is optional for deployment.
            </p>
          )}
        </section>
      </div>
    </>
  );
}
function Application() {
  const location = useLocation();
  if (location.pathname === "/virtual-hardware") return <VirtualHardware />;
  return (
    <WorkspaceProvider>
      <Shell />
    </WorkspaceProvider>
  );
}
const query = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, staleTime: 5000, refetchOnWindowFocus: true },
    mutations: { retry: false },
  },
});
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={query}>
      <BrowserRouter>
        <Application />
      </BrowserRouter>
    </QueryClientProvider>
  </React.StrictMode>,
);
