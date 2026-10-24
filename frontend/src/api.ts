/** Wire contracts mirror Rust serde DTOs. Contract tests use the actual Axum router. */
export type Parameter = number | boolean;
export type Parameters = Record<string, Parameter>;
export type SignalValue =
  { kind: "f64"; value: number } | { kind: "bool"; value: boolean };
export type Mode = "runtime" | "simulation";
export type Matrix = number[][];
export interface SourceDocument {
  source: string;
  revision: string;
  source_revision: string;
}
export interface HistoryWindow {
  samples: Sample[];
  original_count: number;
  downsampled: boolean;
  sample_limit: number;
}
export interface Variables {
  mvs: string[];
  cvs: string[];
  dvs: string[];
}
export interface MpcVariableObjective {
  mode: "maximize" | "minimize" | "minimum_move";
  priority: number;
}
export interface LinearMpcConfig {
  mvs: Record<string, MpcVariableObjective>;
  cvs: Record<string, MpcVariableObjective>;
}
export interface Mv {
  name: string;
  mv_write: string;
  mv_read: string;
  hl_opc?: string | null;
  ll_opc?: string | null;
  hl?: number | null;
  ll?: number | null;
  enable?: string | null;
}
export interface Cv {
  name: string;
  setpoint: string;
  process_value: string;
  optimization_mode_HL?: number | null;
  optimization_mode_LL?: number | null;
  band_low_opc?: string | null;
  band_high_opc?: string | null;
  band_low_value?: number | null;
  band_high_value?: number | null;
  deadband_opc?: string | null;
  deadband_value?: number | null;
  enable?: string | null;
}
export interface Dv {
  name: string;
  read: string;
  enable?: string | null;
}
export interface CustomVariable {
  name: string;
  signal_type: "f64" | "bool";
  read?: string | null;
  write?: string | null;
}
export interface Model {
  kind: "linear_state_space";
  state_count: number;
  u_order: string[];
  y_order: string[];
  a: Matrix;
  b: Matrix;
  c: Matrix;
  d: Matrix;
}
export interface Simulation {
  speed: string;
  cv_setpoints: Record<string, number>;
  cv_initials: Record<string, number>;
  mv_initials: Record<string, number>;
  dv_initials: Record<string, number>;
  cv_noise_std: Record<string, number>;
  mv_noise_std: Record<string, number>;
  dv_disturbances: Record<string, number>;
  cv_offsets: Record<string, number>;
  a_scale_pct: Matrix;
  b_scale_pct: Matrix;
}
export interface NumericRange {
  key: string;
  start: number;
  min: number;
  max: number;
  fixed?: boolean;
}
export interface Optimization {
  parameters: NumericRange[];
  cv_target_steps: Record<string, number>;
  cv_weights: Record<string, number>;
  cv_noise_std: Record<string, number>;
  mv_noise_std: Record<string, number>;
  metric_weights: Record<string, number>;
  a_scale_min_pct: number;
  a_scale_max_pct: number;
  b_scale_min_pct: number;
  b_scale_max_pct: number;
  scenario_count: number;
  max_iterations: number;
  initial_samples: number;
  convergence_patience: number;
  plot_interval: number;
  simulation_duration_s: number;
}
export interface OnlineOptimization {
  parameters: { key: string; min: number; max: number }[];
  cvs: { name: string; weight: number; low: number; high: number }[];
  delta_mvs: { name: string; weight: number }[];
  duration_s: number;
  max_iterations: number;
  initial_samples: number;
  convergence_patience: number;
}
export interface DashboardPanel {
  id: string;
  kind: "mv" | "cv" | "dv" | "custom";
  name: string;
  title: string;
  traces: { direction: string; signal: string }[];
}
export interface DashboardConfig {
  columns: number;
  rows: number;
  panels: DashboardPanel[];
}
export interface PlotConfig {
  dashboard?: DashboardConfig | null;
  trace_colors?: Record<string, string>;
  window_sec: number;
  future_window_sec: number;
  mv_order: string[];
  cv_order: string[];
  dv_order: string[];
  custom_plots: {
    title: string;
    traces: { direction: string; signal: string }[];
  }[];
}
export interface ProjectConfig {
  name: string;
  communication: { protocol: "opc_ua"; endpoint_url: string; cycle_ms: number };
  watchdog:
    | { kind: "none" }
    | {
        kind: "counter_mirror";
        read_node: string;
        write_node: string;
        stale_timeout_ms: number;
      };
  variables: Variables;
  io_definition: { mvs: Mv[]; cvs: Cv[]; dvs: Dv[]; custom: CustomVariable[] };
  io: {
    cycle_ms: number;
    inputs: {
      name: string;
      signal_type: string;
      address: string;
      opc_tag?: string | null;
    }[];
    outputs: {
      name: string;
      signal_type: string;
      address: string;
      opc_tag?: string | null;
    }[];
  };
  limits: {
    values: Record<
      string,
      { min: number; max: number; opc_tag?: string | null }
    >;
  };
  logic: {
    cycle_ms: number;
    kind: "p_controller" | "linear_mpc" | "custom_rust";
    p_controller: {
      pairs: { cv: string; mv: string; kp: number; bias: number }[];
    };
    linear_mpc: LinearMpcConfig;
    custom_rust: {
      strategy_name: string;
      source_path: string;
      artifact_path: string;
      parameter_schema: { name: string; type: string }[];
      required_inputs: string[];
      required_outputs: string[];
      compatible: boolean;
      last_build_message: string;
    };
    params: Parameters;
    initial_values: Record<string, SignalValue>;
  };
  model?: Model | null;
  simulation: Simulation;
  optimization: Optimization;
  online_optimization: OnlineOptimization;
  runtime: Record<string, unknown>;
  alarms: { alarm_timeout_ms: number; off_timeout_ms: number };
  deployment: {
    controller_is_ready?: string | null;
    controller_has_control?: string | null;
    external_controller_enable?: string | null;
    service_name: string;
    display_name: string;
    description: string;
  };
  plots: PlotConfig;
}
export interface Metrics {
  total_cycles: number;
  deadline_misses: number;
  last_duration_us: number;
  last_jitter_us: number;
}
export interface Execution {
  mode: Mode;
  snapshot: {
    execution_id: string;
    project_name: string;
    state: string;
    controller?: {
      is_ready: boolean;
      has_control: boolean;
      enabled: boolean;
      external_enable: boolean;
      observer_converged: boolean;
      reason: string;
    };
    connection_health?: "healthy" | "unhealthy";
    watchdog_status?: string | { fault: { reason: string } };
    metrics: Metrics;
    last_inputs: Record<string, SignalValue>;
    last_outputs: Record<string, SignalValue>;
    simulation_time_s?: number;
  };
}
export interface OnlineStatus {
  phase: "running" | "finished";
  iteration: number;
  max_iterations: number;
  samples_in_window: number;
  samples_per_window: number;
  current_params: Record<string, number>;
  best_params: Record<string, number>;
  best_score: number | null;
  safety_reason: string | null;
  finish_reason: string | null;
}
export interface Workspace {
  execution: Execution | null;
  online_optimization: OnlineStatus | null;
  server_time: string;
}
export interface Sweep {
  kind: "Input" | "Parameter";
  name: string;
  value_kind: "F64" | "Bool";
  baseline_label: string;
  default_min: number | null;
  default_max: number | null;
  default_samples: number;
}
export interface Catalog {
  parameters: Parameters;
  sweeps: Sweep[];
  kalman_parameters: Record<string, number> | null;
  online_optimization_parameters: Record<string, number>;
  cv_safety_limits: Record<string, { min: number; max: number }>;
  simulation_available: boolean;
  simulation_issue: string | null;
  offline_optimization_available: boolean;
}
export interface Readiness {
  ready: boolean;
  issues: { page: string; message: string }[];
  strategy: string;
  model_requirement: string;
  optional: string[];
  hardware_verified: boolean;
  local_command: string;
  windows_service_support: string;
}
export interface Job {
  id: string;
  project_name: string;
  config_revision: string;
  kind: string;
  phase: "running" | "succeeded" | "failed" | "cancelled";
  started_at: string;
  finished_at: string | null;
  cancellable: boolean;
  progress: Record<string, unknown>;
  result: Record<string, unknown> | null;
  error: string | null;
}
export interface Sample {
  timestamp_ms: number;
  direction: string;
  signal: string;
  value: SignalValue;
}
export interface Series {
  direction: string;
  signal: string;
  value_kind: string;
}
export interface SavedProject {
  config: ProjectConfig;
  revision: string;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export async function request<T>(
  path: string,
  body?: unknown,
  method = "GET",
  headers?: HeadersInit,
): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: { "Content-Type": "application/json", ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const text = await response.text();
    let message = text || response.statusText;
    try {
      message = (JSON.parse(text) as { error?: string }).error || message;
    } catch {
      /* Keep server/plain transport diagnostics. */
    }
    throw new ApiError(response.status, message);
  }
  return response.json() as Promise<T>;
}
export const projectPath = (name: string) =>
  `/api/projects/${encodeURIComponent(name)}`;
export async function loadProject(name: string): Promise<SavedProject> {
  const response = await fetch(`${projectPath(name)}/config`);
  if (!response.ok) throw new ApiError(response.status, await response.text());
  return {
    config: (await response.json()) as ProjectConfig,
    revision: response.headers.get("etag") || "",
  };
}
export function download(name: string, content: string, type = "text/plain") {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
