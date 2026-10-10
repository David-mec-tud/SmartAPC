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
  type SourceDocument,
  type Workspace,
} from "./api";
interface Draft extends SavedProject {
  original: string;
  autoSaveLogic?: boolean;
}
interface SourceDraft extends SourceDocument {
  original: string;
}
// Rust maps have no serialized key order. Compare values canonically so a
// refreshed map does not manufacture an unsaved draft after a scoped save.
function configurationFingerprint(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) => {
    if (item && typeof item === "object" && !Array.isArray(item))
      return Object.fromEntries(
        Object.entries(item).sort(([a], [b]) => a.localeCompare(b)),
      );
    return item;
  });
}
function useWorkspaceState() {
  const client = useQueryClient();
  const [project, setProject] = useState(
    sessionStorage.getItem("project") || "",
  );
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  // Keep source text owned by its project across route changes and selection changes.
  const [sources, setSources] = useState<Record<string, SourceDraft>>({});
  const [notification, setNotification] = useState("");
  const [streamConnected, setStreamConnected] = useState(false);
  const [syncingJob, setSyncingJob] = useState(false);
  const [strategyFailures, setStrategyFailures] = useState<
    Record<string, { submitted: string; message: string }>
  >({});
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
  const terminalJobs = jobs.data
    ?.filter((job) => job.phase !== "running")
    .map((job) => `${job.id}:${job.finished_at}`)
    .join(",");
  useEffect(() => {
    if (terminalJobs === undefined) return;
    let disposed = false;
    setSyncingJob(true);
    // Builds update saved compatibility and schemas. Refresh before unlocking
    // editors, so the next edit starts from the build's authoritative revision.
    void Promise.all([
      client.invalidateQueries({ queryKey: ["project", project] }),
      client.invalidateQueries({ queryKey: ["catalog", project] }),
      client.invalidateQueries({ queryKey: ["readiness", project] }),
    ]).finally(() => {
      if (!disposed) setSyncingJob(false);
    });
    return () => {
      disposed = true;
    };
  }, [client, project, terminalJobs]);
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
    !!current && configurationFingerprint(current.config) !== current.original;
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      if (
        Object.values(drafts).some(
          (draft) => configurationFingerprint(draft.config) !== draft.original,
        ) ||
        Object.values(sources).some(
          (source) => source.source !== source.original,
        )
      )
        event.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [drafts, sources]);
  async function loadSource(discardDraft = false) {
    const name = project;
    const requestedText = sources[name]?.source;
    try {
      const source = await request<SourceDocument>(
        `${projectPath(name)}/strategy/source`,
      );
      setSources((previous) => {
        // A delayed automatic/template load must never replace text typed while
        // the request was in flight. Explicit Reload remains the discard action.
        const current = previous[name];
        if (
          current &&
          current.source !== current.original &&
          (!discardDraft || current.source !== requestedText)
        )
          return previous;
        return { ...previous, [name]: { ...source, original: source.source } };
      });
    } catch (error) {
      setNotification((error as Error).message);
    }
  }
  function editSource(text: string) {
    setSources((previous) => {
      const source = previous[project];
      return source
        ? { ...previous, [project]: { ...source, source: text } }
        : previous;
    });
  }
  async function saveSource() {
    const name = project;
    const submitted = sources[name];
    if (!submitted) return;
    try {
      const result = await request<{
        revision: string;
        source_revision: string;
      }>(`${projectPath(name)}/strategy/source`, submitted, "PUT");
      setSources((previous) => ({
        ...previous,
        [name]: {
          ...previous[name],
          ...result,
          original: submitted.source,
        },
      }));
      await client.invalidateQueries({ queryKey: ["project", name] });
      setNotification(
        "Source saved. Build and check its contract before running.",
      );
    } catch (error) {
      setNotification((error as Error).message);
    }
  }
  function edit(update: (cfg: ProjectConfig) => void, autoSaveLogic = false) {
    if (!config || !saved.data) return;
    const next = structuredClone(config);
    update(next);
    setDrafts((previous) => ({
      ...previous,
      [project]: {
        config: next,
        revision: current?.revision ?? saved.data.revision,
        original:
          current?.original ?? configurationFingerprint(saved.data.config),
        autoSaveLogic: autoSaveLogic || current?.autoSaveLogic,
      },
    }));
  }
  function editStrategy(update: (cfg: ProjectConfig) => void) {
    edit(update, true);
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
      const submitted = configurationFingerprint(config);
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
        if (
          draft &&
          configurationFingerprint(draft.config) === result.submitted
        )
          delete next[result.name];
        else if (draft)
          next[result.name] = {
            ...draft,
            revision: result.saved.revision,
            original: configurationFingerprint(result.saved.config),
          };
        return next;
      });
      await refresh();
      setNotification("Configuration saved and normalized by Rust.");
    },
    onError: (error) => setNotification(error.message),
  });
  const strategySave = useMutation({
    mutationFn: async ({ name, draft }: { name: string; draft: Draft }) => ({
      name,
      revision: draft.revision,
      submitted: configurationFingerprint(draft.config.logic),
      saved: await request<SavedProject>(
        `${projectPath(name)}/strategy/config`,
        { revision: draft.revision, config: draft.config.logic },
        "PUT",
      ),
    }),
    onSuccess: async (result) => {
      client.setQueryData(["project", result.name], result.saved);
      setDrafts((previous) => {
        const draft = previous[result.name];
        if (!draft) return previous;
        const original = JSON.parse(draft.original) as ProjectConfig;
        const nextConfig = { ...draft.config };
        const newerLogic =
          configurationFingerprint(draft.config.logic) !== result.submitted;
        // Accept Rust normalization only for unchanged sections. Preserve both
        // unrelated drafts and edits typed while this request was in flight.
        for (const section of Object.keys(
          nextConfig,
        ) as (keyof ProjectConfig)[]) {
          if (
            section === "logic"
              ? !newerLogic
              : configurationFingerprint(nextConfig[section]) ===
                configurationFingerprint(original[section])
          )
            Object.assign(nextConfig, {
              [section]: result.saved.config[section],
            });
        }
        if (
          configurationFingerprint(nextConfig) ===
          configurationFingerprint(result.saved.config)
        ) {
          const next = { ...previous };
          delete next[result.name];
          return next;
        }
        return {
          ...previous,
          [result.name]: {
            config: nextConfig,
            revision: result.saved.revision,
            original: configurationFingerprint(result.saved.config),
            autoSaveLogic: newerLogic,
          },
        };
      });
      // Our own logic save did not change source bytes. Advance its configuration
      // revision so a retained source draft can still be saved after switching strategies.
      setSources((previous) => {
        const source = previous[result.name];
        return source?.revision === result.revision
          ? {
              ...previous,
              [result.name]: { ...source, revision: result.saved.revision },
            }
          : previous;
      });
      await Promise.all([
        client.invalidateQueries({ queryKey: ["catalog", result.name] }),
        client.invalidateQueries({ queryKey: ["readiness", result.name] }),
      ]);
    },
    onError: (error, { name, draft }) => {
      setStrategyFailures((previous) => ({
        ...previous,
        [name]: {
          submitted: configurationFingerprint(draft.config.logic),
          message: error.message,
        },
      }));
    },
  });
  useEffect(() => {
    if (save.isPending || strategySave.isPending || syncingJob) return;
    const pending = Object.entries(drafts).find(
      ([name, draft]) =>
        draft.autoSaveLogic &&
        workspace.data?.execution?.snapshot.project_name !== name &&
        !(
          name === project && jobs.data?.some((job) => job.phase === "running")
        ) &&
        strategyFailures[name]?.submitted !==
          configurationFingerprint(draft.config.logic),
    );
    if (!pending) return;
    // The provider owns this debounce: navigation and project switching cannot
    // cancel the last edit. Serialize saves to avoid competing revision writes.
    const timer = window.setTimeout(() => {
      strategySave.mutate({ name: pending[0], draft: pending[1] });
    }, 500);
    return () => window.clearTimeout(timer);
  }, [
    drafts,
    save.isPending,
    strategySave.isPending,
    strategySave.mutate,
    syncingJob,
    project,
    jobs.data,
    workspace.data,
    strategyFailures,
  ]);
  const strategyError =
    current?.autoSaveLogic &&
    strategyFailures[project]?.submitted ===
      configurationFingerprint(current.config.logic)
      ? strategyFailures[project].message
      : undefined;
  const strategyPending =
    !!current?.autoSaveLogic ||
    (strategySave.isPending && strategySave.variables?.name === project);
  function retryStrategySave() {
    setStrategyFailures((previous) => {
      const next = { ...previous };
      delete next[project];
      return next;
    });
  }
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
          original: configurationFingerprint(authoritative.config),
        },
      };
    });
  }
  const execution = workspace.data?.execution ?? null;
  const executingProject = execution?.snapshot.project_name;
  const busyJob = jobs.data?.some((job) => job.phase === "running");
  const locked = executingProject === project || !!busyJob || syncingJob;
  return {
    project,
    setProject,
    projects,
    saved,
    config,
    edit,
    editStrategy,
    strategyPending,
    strategyError,
    retryStrategySave,
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
    source: sources[project],
    sourceDirty:
      !!sources[project] &&
      sources[project].source !== sources[project].original,
    loadSource,
    editSource,
    saveSource,
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
