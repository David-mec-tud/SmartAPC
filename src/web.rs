use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    deployment::{DeploymentCommands, windows_service_commands},
    project::ProjectService,
    project_config::ProjectConfig,
    runtime::{RuntimeManager, RuntimeSnapshot},
    storage::SignalSample,
    types::SignalValue,
};

#[derive(Clone)]
pub struct AppState {
    pub project_service: ProjectService,
    pub runtime: Arc<RuntimeManager>,
    pub executable_path: PathBuf,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{name}/config",
            get(get_project_config).put(update_project_config),
        )
        .route("/api/projects/{name}/sidebar", get(get_sidebar_tasks))
        .route("/api/runtime/start", post(start_runtime))
        .route("/api/runtime/stop", post(stop_runtime))
        .route("/api/runtime/state", get(get_runtime_state))
        .route(
            "/api/runtime/params",
            get(get_runtime_params).put(set_runtime_params),
        )
        .route("/api/plots", get(get_plots))
        .route("/api/simulate/step", post(simulate_step))
        .route("/api/deployment/commands", get(get_deployment_commands))
        .with_state(state)
}

pub async fn serve(state: AppState, bind: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed binding web server at {bind}"))?;

    axum::serve(listener, build_router(state))
        .await
        .context("web server failed")
}

async fn index_page() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>SmartAPC V1</title>
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <style>
    :root {
      --bg: #f3f7fb;
      --panel: #ffffff;
      --text: #1a2b3c;
      --muted: #5a6f85;
      --accent: #0f7b6c;
      --border: #d9e3ee;
    }
    body {
      margin: 0;
      font-family: "Source Sans 3", "Segoe UI", sans-serif;
      background:
        radial-gradient(circle at 15% 5%, #d2ebe8 0%, transparent 30%),
        radial-gradient(circle at 85% 0%, #e7e5f9 0%, transparent 25%),
        var(--bg);
      color: var(--text);
    }
    .container { max-width: 960px; margin: 48px auto; padding: 0 20px; }
    .panel { background: var(--panel); border: 1px solid var(--border); border-radius: 16px; padding: 24px; box-shadow: 0 14px 30px rgba(15, 27, 45, 0.07); }
    h1 { margin: 0 0 10px; font-size: 30px; }
    p { margin: 0 0 14px; color: var(--muted); }
    code { background: #ecf3fa; padding: 2px 6px; border-radius: 6px; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 16px; margin-top: 18px; }
    .card { border: 1px solid var(--border); border-radius: 12px; padding: 16px; }
    .card h3 { margin: 0 0 8px; }
  </style>
</head>
<body>
  <div class="container">
    <div class="panel">
      <h1>SmartAPC V1</h1>
      <p>Modulare Regelungssoftware mit OPC-UA, Watchdog, Runtime-State-Machine und SQLite-Plots.</p>
      <p>API Startpunkte: <code>/api/projects</code>, <code>/api/runtime/start</code>, <code>/api/runtime/state</code>.</p>
      <div class="grid">
        <div class="card"><h3>Projektkonfiguration</h3><p>TOML-basierte Projekte, Sidebar-Tasks und Parametrierung.</p></div>
        <div class="card"><h3>Runtime</h3><p>State: Running -> Hold -> Alarm -> Off mit konfigurierbaren Timeouts.</p></div>
        <div class="card"><h3>Plots</h3><p>Ein-/Ausgänge und Zyklus-Metriken werden lokal in SQLite gespeichert.</p></div>
      </div>
    </div>
  </div>
</body>
</html>"#,
    )
}

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    name: String,
}

async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<ProjectConfig>, AppError> {
    let cfg = state.project_service.create_project(&req.name)?;
    Ok(Json(cfg))
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    Ok(Json(state.project_service.list_projects()?))
}

async fn get_project_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ProjectConfig>, AppError> {
    let cfg = state.project_service.load_config(&name)?;
    Ok(Json(cfg))
}

async fn update_project_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(cfg): Json<ProjectConfig>,
) -> Result<Json<ProjectConfig>, AppError> {
    state.project_service.save_config(&name, &cfg)?;
    Ok(Json(cfg))
}

async fn get_sidebar_tasks(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tasks = state.project_service.sidebar_tasks(&name)?;
    Ok(Json(serde_json::json!({ "tasks": tasks })))
}

#[derive(Debug, Deserialize)]
struct StartRuntimeRequest {
    project_name: String,
}

async fn start_runtime(
    State(state): State<AppState>,
    Json(req): Json<StartRuntimeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .runtime
        .start(&state.project_service, &req.project_name)
        .await?;
    Ok(Json(
        serde_json::json!({"status": "started", "project": req.project_name}),
    ))
}

async fn stop_runtime(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    state.runtime.stop().await?;
    Ok(Json(serde_json::json!({"status": "stopped"})))
}

async fn get_runtime_state(
    State(state): State<AppState>,
) -> Result<Json<Option<RuntimeSnapshot>>, AppError> {
    Ok(Json(state.runtime.state().await?))
}

async fn get_runtime_params(
    State(state): State<AppState>,
) -> Result<Json<HashMap<String, f64>>, AppError> {
    let params = state.runtime.get_params().await?;
    Ok(Json(params))
}

#[derive(Debug, Deserialize)]
struct SetParamsRequest {
    params: HashMap<String, f64>,
}

async fn set_runtime_params(
    State(state): State<AppState>,
    Json(req): Json<SetParamsRequest>,
) -> Result<Json<HashMap<String, f64>>, AppError> {
    let params = state.runtime.set_params(req.params.clone()).await?;

    if let Some(project_name) = state.runtime.active_project().await {
        state
            .project_service
            .update_logic_params(&project_name, req.params)?;
    }

    Ok(Json(params))
}

#[derive(Debug, Deserialize)]
struct PlotQuery {
    limit: Option<usize>,
}

async fn get_plots(
    State(state): State<AppState>,
    Query(query): Query<PlotQuery>,
) -> Result<Json<Vec<SignalSample>>, AppError> {
    let limit = query.limit.unwrap_or(250).clamp(1, 10_000);
    let rows = state.runtime.read_plots(limit).await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct SimulateRequest {
    inputs: HashMap<String, SignalValue>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SimulateResponse {
    outputs: HashMap<String, SignalValue>,
}

async fn simulate_step(
    State(state): State<AppState>,
    Json(req): Json<SimulateRequest>,
) -> Result<Json<SimulateResponse>, AppError> {
    let outputs = state.runtime.simulate_step(req.inputs).await?;
    Ok(Json(SimulateResponse { outputs }))
}

#[derive(Debug, Deserialize)]
struct DeploymentQuery {
    project: String,
}

async fn get_deployment_commands(
    State(state): State<AppState>,
    Query(query): Query<DeploymentQuery>,
) -> Result<Json<DeploymentCommands>, AppError> {
    let cfg = state.project_service.load_config(&query.project)?;
    let commands = windows_service_commands(
        &cfg.deployment,
        &state.executable_path,
        &query.project,
        state.project_service.root(),
    );
    Ok(Json(commands))
}

#[derive(Debug)]
struct AppError(anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self(value.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let msg = self.0.to_string();
        let status = if msg.contains("not running") || msg.contains("not found") {
            StatusCode::NOT_FOUND
        } else if msg.contains("already") || msg.contains("invalid") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };

        (
            status,
            Json(serde_json::json!({
                "error": msg,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum_test::TestServer;
    use tempfile::tempdir;

    use crate::{
        project::ProjectService,
        project_config::{MappingRule, SignalPoint, WatchdogConfig},
        runtime::RuntimeManager,
        types::{SignalType, SignalValue},
    };

    use super::*;

    #[tokio::test]
    async fn project_api_roundtrip_and_param_persist() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        let runtime = Arc::new(RuntimeManager::new());
        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime,
            executable_path: std::env::current_exe().expect("exe"),
        });

        let server = TestServer::new(app).expect("server");

        let create_resp = server
            .post("/api/projects")
            .json(&serde_json::json!({"name": "demo"}))
            .await;
        create_resp.assert_status_ok();

        let mut cfg = projects.load_config("demo").expect("config");
        cfg.communication.endpoint_url = "mock://web-project-api".to_string();
        cfg.io.inputs.push(SignalPoint {
            name: "pv".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=PV".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=MV".to_string(),
            opc_tag: None,
        });
        cfg.watchdog = WatchdogConfig::None;
        cfg.logic.mapping_rules.push(MappingRule {
            input: "pv".to_string(),
            output: "mv".to_string(),
            gain: 1.0,
            offset: 0.0,
            invert_bool: false,
        });
        projects.save_config("demo", &cfg).expect("save cfg");

        let start_resp = server
            .post("/api/runtime/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await;
        start_resp.assert_status_ok();

        let update_resp = server
            .put("/api/runtime/params")
            .json(&serde_json::json!({
                "params": { "mv.gain": 2.5 }
            }))
            .await;
        update_resp.assert_status_ok();

        let cfg2 = projects.load_config("demo").expect("reload cfg");
        assert_eq!(cfg2.logic.params.get("mv.gain").copied(), Some(2.5));

        let sim_resp = server
            .post("/api/simulate/step")
            .json(&serde_json::json!({
                "inputs": {
                    "pv": { "kind": "f64", "value": 3.0 }
                }
            }))
            .await;
        sim_resp.assert_status_ok();

        let body: SimulateResponse = sim_resp.json();
        assert_eq!(body.outputs.get("mv"), Some(&SignalValue::F64(7.5)));

        let _ = server.post("/api/runtime/stop").await;
    }

    #[tokio::test]
    async fn sidebar_endpoint_exposes_tasks() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let app = build_router(AppState {
            project_service: projects,
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        let resp = server.get("/api/projects/demo/sidebar").await;
        resp.assert_status_ok();

        let json: serde_json::Value = resp.json();
        let tasks = json
            .get("tasks")
            .and_then(|v| v.as_array())
            .expect("tasks array");

        assert!(!tasks.is_empty());
    }

    #[tokio::test]
    async fn plots_endpoint_reads_samples() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let mut cfg = projects.load_config("demo").expect("config");
        cfg.communication.endpoint_url = "mock://web-plots".to_string();
        cfg.io.inputs.push(SignalPoint {
            name: "in1".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=In1".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "out1".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Out1".to_string(),
            opc_tag: None,
        });
        cfg.logic.mapping_rules.push(MappingRule {
            input: "in1".to_string(),
            output: "out1".to_string(),
            gain: 1.0,
            offset: 0.0,
            invert_bool: false,
        });
        projects.save_config("demo", &cfg).expect("save cfg");

        let app = build_router(AppState {
            project_service: projects.clone(),
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        server
            .post("/api/runtime/start")
            .json(&serde_json::json!({"project_name": "demo"}))
            .await
            .assert_status_ok();

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

        let plots_resp = server.get("/api/plots?limit=10").await;
        plots_resp.assert_status_ok();

        let samples: Vec<SignalSample> = plots_resp.json();
        assert!(!samples.is_empty());

        let _ = server.post("/api/runtime/stop").await;
    }

    #[tokio::test]
    async fn deployment_commands_endpoint_returns_windows_commands() {
        let dir = tempdir().expect("tmp");
        let projects = ProjectService::new(dir.path());
        projects.create_project("demo").expect("create");

        let app = build_router(AppState {
            project_service: projects,
            runtime: Arc::new(RuntimeManager::new()),
            executable_path: std::env::current_exe().expect("exe"),
        });
        let server = TestServer::new(app).expect("server");

        let resp = server.get("/api/deployment/commands?project=demo").await;
        resp.assert_status_ok();

        let body: DeploymentCommands = resp.json();
        assert!(body.install.contains("sc.exe create"));
        assert!(body.start.contains("sc.exe start"));
    }
}
