import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  loadProject,
  projectPath,
  request,
  type Catalog,
  type Job,
  type ProjectConfig,
  type SavedProject,
  type Workspace,
} from "./api";
interface Draft extends SavedProject {
  original: string;
}
function useWorkspaceState() {
  const client = useQueryClient();
  const [project, setProject] = useState(
    sessionStorage.getItem("project") || "",
  );
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [notification, setNotification] = useState("");
  const [streamConnected, setStreamConnected] = useState(false);
  const projects = useQuery({
    queryKey: ["projects"],
    queryFn: () => request<string[]>("/api/projects"),
  });
  const saved = useQuery({
    queryKey: ["project", project],
    queryFn: () => loadProject(project),
    enabled: !!project,
    refetchInterval: 5000,
  });
  const workspace = useQuery({
    queryKey: ["workspace"],
    queryFn: () => request<Workspace>("/api/workspace"),
    refetchInterval: streamConnected ? 5000 : 1500,
    retry: 1,
  });
  const catalog = useQuery({
    queryKey: ["catalog", project],
    queryFn: () => request<Catalog>(`${projectPath(project)}/engineering`),
    enabled: !!project,
  });
  const jobs = useQuery({
    queryKey: ["jobs", project],
    queryFn: () => request<Job[]>(`${projectPath(project)}/jobs`),
    enabled: !!project,
    refetchInterval: 1500,
  });
  useEffect(() => {
    const events = new EventSource("/api/events");
    events.addEventListener("workspace", (event) => {
      client.setQueryData(
        ["workspace"],
        JSON.parse((event as MessageEvent).data),
      );
      setStreamConnected(true);
    });
    events.onerror = () => setStreamConnected(false);
    return () => events.close();
  }, [client]);
  useEffect(() => {
    if (!project && projects.data?.length) setProject(projects.data[0]);
  }, [project, projects.data]);
  useEffect(() => {
    sessionStorage.setItem("project", project);
  }, [project]);
  const current = drafts[project];
  const config = current?.config ?? saved.data?.config;
  const dirty =
    !!current && JSON.stringify(current.config) !== current.original;
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      if (
        Object.values(drafts).some(
          (draft) => JSON.stringify(draft.config) !== draft.original,
        )
      )
        event.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [drafts]);
  function edit(update: (cfg: ProjectConfig) => void) {
    if (!config || !saved.data) return;
    const next = structuredClone(config);
    update(next);
    setDrafts((previous) => ({
      ...previous,
      [project]: {
        config: next,
        revision: current?.revision ?? saved.data.revision,
        original: current?.original ?? JSON.stringify(saved.data.config),
      },
    }));
  }
  function discard() {
    setDrafts((previous) => {
      const next = { ...previous };
      delete next[project];
      return next;
    });
  }
  const save = useMutation({
    mutationFn: async () => {
      if (!config || !saved.data) throw Error("No project selected");
      const name = project;
      const submitted = JSON.stringify(config);
      await request<ProjectConfig>(
        `${projectPath(name)}/config`,
        config,
        "PUT",
        { "If-Match": current?.revision ?? saved.data.revision },
      );
      return { name, submitted, saved: await loadProject(name) };
    },
    onSuccess: async (result) => {
      client.setQueryData(["project", result.name], result.saved);
      setDrafts((previous) => {
        const next = { ...previous };
        const draft = next[result.name];
        if (draft && JSON.stringify(draft.config) === result.submitted)
          delete next[result.name];
        else if (draft)
          next[result.name] = {
            ...draft,
            revision: result.saved.revision,
            original: JSON.stringify(result.saved.config),
          };
        return next;
      });
      await refresh();
      setNotification("Configuration saved and normalized by Rust.");
    },
    onError: (error) => setNotification(error.message),
  });
  async function refresh() {
    await Promise.all([
      client.invalidateQueries({ queryKey: ["projects"] }),
      client.invalidateQueries({ queryKey: ["project", project] }),
      client.invalidateQueries({ queryKey: ["catalog", project] }),
      client.invalidateQueries({ queryKey: ["jobs", project] }),
      client.invalidateQueries({ queryKey: ["readiness", project] }),
      client.invalidateQueries({ queryKey: ["workspace"] }),
    ]);
  }
  async function act(path: string, body?: unknown, method = "POST") {
    try {
      const result = await request<unknown>(path, body, method);
      await refresh();
      return result;
    } catch (error) {
      setNotification((error as Error).message);
      throw error;
    }
  }
  async function startJob(body: unknown) {
    const result = await act(`${projectPath(project)}/jobs`, body);
    setNotification(
      "Engineering job started. Its progress and results remain available here.",
    );
    return result;
  }
  async function acceptSavedSection(section: keyof ProjectConfig) {
    const name = project;
    const authoritative = await loadProject(name);
    client.setQueryData(["project", name], authoritative);
    setDrafts((previous) => {
      const draft = previous[name];
      if (!draft) return previous;
      const config = {
        ...draft.config,
        [section]: authoritative.config[section],
      };
      return {
        ...previous,
        [name]: {
          config,
          revision: authoritative.revision,
          original: JSON.stringify(authoritative.config),
        },
      };
    });
  }
  const execution = workspace.data?.execution ?? null;
  const executingProject = execution?.snapshot.project_name;
  const busyJob = jobs.data?.some((job) => job.phase === "running");
  const locked = executingProject === project || !!busyJob;
  return {
    project,
    setProject,
    projects,
    saved,
    config,
    edit,
    dirty,
    discard,
    save,
    refresh,
    workspace,
    execution,
    catalog,
    jobs,
    locked,
    notification,
    setNotification,
    act,
    startJob,
    acceptSavedSection,
  };
}
type State = ReturnType<typeof useWorkspaceState>;
const Context = createContext<State | null>(null);
export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const value = useWorkspaceState();
  return <Context.Provider value={value}>{children}</Context.Provider>;
}
export function useWorkspace() {
  const value = useContext(Context);
  if (!value) throw Error("Workspace provider is missing");
  return value;
}
