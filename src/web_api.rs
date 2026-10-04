//! Typed browser workflows layered over existing Rust controller services.
use crate::{
    engineering::{
        EngineeringService, JobRequest, JobSnapshot, config_revision, content_revision,
        engineering_catalog, project_readiness,
    },
    parameter_optimization::apply_optimized_parameters,
    project_config::{LogicParamValue, SimulationConfig, from_toml_str, to_toml_string},
    storage::SqliteStorage,
    strategy_service,
    web::{AppError, AppState},
};
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{
        IntoResponse, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    convert::Infallible,
    io::{Read, Seek, SeekFrom},
    time::Duration,
};
use tokio_stream::wrappers::ReceiverStream;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/events", get(events))
        .route("/api/workspace", get(workspace))
        .route("/api/projects/{name}/engineering", get(catalog))
        .route("/api/projects/{name}/readiness", get(readiness))
        .route(
            "/api/projects/{name}/online-optimization/config",
            axum::routing::put(save_online_config),
        )
        .route("/api/projects/{name}/config/download", get(download_config))
        .route("/api/projects/{name}/config/parse", post(parse_config))
        .route("/api/projects/{name}/jobs", get(list_jobs).post(start_job))
        .route("/api/projects/{name}/jobs/{id}", get(get_job))
        .route("/api/projects/{name}/jobs/{id}/cancel", post(cancel_job))
        .route("/api/projects/{name}/jobs/{id}/apply", post(apply_job))
        .route(
            "/api/projects/{name}/strategy/template",
            post(generate_template),
        )
        .route(
            "/api/projects/{name}/strategy/source",
            get(get_source).put(save_source),
        )
        .route("/api/projects/{name}/logs", get(logs))
        .route("/api/projects/{name}/history", get(history))
        .route("/api/projects/{name}/history/catalog", get(history_catalog))
        .route(
            "/api/execution/parameters",
            get(get_execution_params).post(set_execution_params),
        )
        .route("/api/execution/command", post(execution_command))
        .route("/api/execution/simulation-config", post(simulation_config))
        .route(
            "/api/execution/optimization",
            get(optimization_status).post(optimization_command),
        )
}

async fn workspace(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(
        json!({"execution": state.runtime.active_state().await?, "online_optimization": state.runtime.online_optimization_status().await?, "server_time": chrono::Utc::now().to_rfc3339()}),
    ))
}

async fn events(State(state): State<AppState>) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(2);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let payload = match workspace(State(state.clone())).await {
                Ok(Json(payload)) => payload,
                Err(err) => json!({"error": err.to_string()}),
            };
            let event = Event::default()
                .event("workspace")
                .data(payload.to_string());
            if tx.send(Ok(event)).await.is_err() {
                break;
            }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
}

async fn catalog(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(engineering_catalog(
        &state.project_service.load_config(&name)?,
    )?))
}

async fn readiness(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let cfg = state.project_service.load_config(&name)?;
    let mut response = project_readiness(&state.project_service, &name, &cfg);
    response["local_command"] = json!(format!(
        "\"{}\" serve --project-root \"{}\" --project \"{}\"",
        state.executable_path.display(),
        state.project_service.root().display(),
        name
    ));
    Ok(Json(response))
}

async fn download_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let cfg = state.project_service.load_config(&name)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/toml".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}.toml\""),
            ),
        ],
        to_toml_string(&cfg)?,
    ))
}

#[derive(Deserialize)]
struct ParseConfig {
    toml: String,
}
async fn parse_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<ParseConfig>,
) -> Result<Json<Value>, AppError> {
    state.project_service.load_config(&name)?;
    let mut cfg = from_toml_str(&request.toml)?;
    cfg.name = name;
    // Parsing stages an editable draft; saving remains a separate revision-checked action.
    Ok(Json(serde_json::to_value(cfg)?))
}

async fn list_jobs(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path(name): Path<String>,
) -> Result<Json<Vec<JobSnapshot>>, AppError> {
    state.project_service.load_config(&name)?;
    Ok(Json(engineering.list(&state.project_service, &name)?))
}

async fn start_job(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path(name): Path<String>,
    Json(request): Json<JobRequest>,
) -> Result<(StatusCode, Json<JobSnapshot>), AppError> {
    let _mutation = engineering.mutations.lock().await;
    engineering.require_editable(&state.runtime, &name).await?;
    let snapshot = engineering.start(state.project_service, name, request)?;
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

async fn get_job(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<JobSnapshot>, AppError> {
    state.project_service.load_config(&name)?;
    Ok(Json(engineering.get(&state.project_service, &name, &id)?))
}

async fn cancel_job(
    Extension(engineering): Extension<EngineeringService>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    engineering.cancel(&name, &id)?;
    Ok(Json(json!({"status": "cancellation_requested"})))
}

async fn apply_job(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path((name, id)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    engineering.require_editable(&state.runtime, &name).await?;
    let job = engineering.get(&state.project_service, &name, &id)?;
    if job.kind != "offline_optimization" || job.result.is_none() {
        return Err(AppError::status(
            StatusCode::BAD_REQUEST,
            "this job has no applicable optimization result",
        ));
    }
    if job.config_revision != config_revision(&state.project_service, &name)? {
        return Err(AppError::status(
            StatusCode::PRECONDITION_FAILED,
            "project changed since this optimization; rerun before applying its results",
        ));
    }
    let params =
        serde_json::from_value(job.result.context("job result missing")?["best_params"].clone())?;
    Ok(Json(serde_json::to_value(apply_optimized_parameters(
        &state.project_service,
        &name,
        &params,
    )?)?))
}

async fn generate_template(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    engineering.require_editable(&state.runtime, &name).await?;
    Ok(Json(serde_json::to_value(
        strategy_service::generate_template(&state.project_service, &name)?,
    )?))
}

#[derive(Serialize, Deserialize)]
struct SourceDraft {
    source: String,
    revision: String,
    source_revision: String,
}
async fn get_source(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SourceDraft>, AppError> {
    let source = std::fs::read_to_string(strategy_service::source_path(
        &state.project_service,
        &name,
    )?)?;
    Ok(Json(SourceDraft {
        source_revision: content_revision(source.as_bytes()),
        source,
        revision: config_revision(&state.project_service, &name)?,
    }))
}

async fn save_source(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path(name): Path<String>,
    Json(request): Json<SourceDraft>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    engineering.require_editable(&state.runtime, &name).await?;
    if request.revision != config_revision(&state.project_service, &name)? {
        return Err(AppError::status(
            StatusCode::PRECONDITION_FAILED,
            "project changed; reload before saving source",
        ));
    }
    let path = strategy_service::source_path(&state.project_service, &name)?;
    if request.source_revision != content_revision(&std::fs::read(&path)?) {
        return Err(AppError::status(
            StatusCode::PRECONDITION_FAILED,
            "source changed in another editor; reload before saving this draft",
        ));
    }
    // Replace the file atomically so a build or an external reader sees a complete source.
    let pending = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&pending, &request.source)?;
    std::fs::rename(&pending, path)?;
    let mut cfg = state.project_service.load_config(&name)?;
    cfg.logic.custom_rust.compatible = false;
    cfg.logic.custom_rust.last_build_message =
        "Source changed. Build and check before running.".into();
    state.project_service.save_config(&name, &cfg)?;
    Ok(Json(
        json!({"saved": true, "revision": config_revision(&state.project_service, &name)?, "source_revision": content_revision(request.source.as_bytes())}),
    ))
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ExecutionMode {
    Runtime,
    Simulation,
}
impl ExecutionMode {
    fn label(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Simulation => "simulation",
        }
    }
}

#[derive(Deserialize)]
struct ExecutionIdentity {
    project_name: String,
    mode: ExecutionMode,
    #[serde(default)]
    execution_id: Option<String>,
}
async fn require_execution(state: &AppState, request: &ExecutionIdentity) -> Result<(), AppError> {
    let snapshot = serde_json::to_value(state.runtime.active_state().await?)?;
    if snapshot["snapshot"]["project_name"] != request.project_name
        || snapshot["mode"] != request.mode.label()
        || snapshot["snapshot"]["execution_id"].as_str() != request.execution_id.as_deref()
    {
        return Err(AppError::status(
            StatusCode::CONFLICT,
            "request project, mode or execution ID does not match active execution",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct ParameterUpdate {
    #[serde(flatten)]
    identity: ExecutionIdentity,
    params: HashMap<String, LogicParamValue>,
    expected_params: HashMap<String, LogicParamValue>,
}

async fn get_execution_params(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Query(identity): Query<ExecutionIdentity>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    require_execution(&state, &identity).await?;
    Ok(Json(
        json!({"project_name": identity.project_name, "mode": identity.mode.label(),
        "execution_id": identity.execution_id, "params": state.runtime.active_get_params().await?}),
    ))
}
async fn set_execution_params(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Json(request): Json<ParameterUpdate>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    require_execution(&state, &request.identity).await?;
    let saved = state
        .project_service
        .load_config(&request.identity.project_name)?;
    let mut candidate = saved.clone();
    candidate.sync_active_online_params();
    for (key, value) in &request.params {
        let current = candidate
            .logic
            .params
            .get(key)
            .context("unknown active online parameter")?;
        if !current.matches_type(value.param_type()) {
            return Err(AppError::status(
                StatusCode::BAD_REQUEST,
                "online parameter type mismatch",
            ));
        }
        candidate.logic.params.insert(key.clone(), value.clone());
    }
    candidate.validate()?;
    let current = state.runtime.active_get_params().await?;
    if request
        .params
        .keys()
        .any(|key| request.expected_params.get(key) != current.get(key))
    {
        return Err(AppError::status(
            StatusCode::PRECONDITION_FAILED,
            "applied parameter changed; refresh before applying this draft",
        ));
    }
    let applied = state.runtime.active_set_params(request.params).await?;
    state
        .project_service
        .save_config(&request.identity.project_name, &candidate)?;
    Ok(Json(
        json!({"project_name": request.identity.project_name, "mode": request.identity.mode.label(), "params": applied}),
    ))
}

#[derive(Deserialize)]
struct ExecutionCommand {
    #[serde(flatten)]
    identity: ExecutionIdentity,
    action: String,
}
async fn execution_command(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Json(request): Json<ExecutionCommand>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    match request.action.as_str() {
        "start" => {
            engineering
                .require_editable(&state.runtime, &request.identity.project_name)
                .await?;
            match request.identity.mode {
                ExecutionMode::Runtime => {
                    state
                        .runtime
                        .start(&state.project_service, &request.identity.project_name)
                        .await?
                }
                ExecutionMode::Simulation => {
                    state
                        .runtime
                        .start_simulation(&state.project_service, &request.identity.project_name)
                        .await?
                }
            }
        }
        "stop" => {
            require_execution(&state, &request.identity).await?;
            match request.identity.mode {
                ExecutionMode::Runtime => state.runtime.stop().await?,
                ExecutionMode::Simulation => state.runtime.stop_simulation().await?,
            }
        }
        _ => {
            return Err(AppError::status(
                StatusCode::BAD_REQUEST,
                "unknown execution action",
            ));
        }
    }
    Ok(Json(
        json!({"execution": state.runtime.active_state().await?}),
    ))
}

#[derive(Deserialize)]
struct SimulationUpdate {
    #[serde(flatten)]
    identity: ExecutionIdentity,
    config: SimulationConfig,
}
async fn simulation_config(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Json(request): Json<SimulationUpdate>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    require_execution(&state, &request.identity).await?;
    if !matches!(request.identity.mode, ExecutionMode::Simulation) {
        return Err(AppError::status(
            StatusCode::CONFLICT,
            "active execution is not a simulation",
        ));
    }
    let mut cfg = state
        .project_service
        .load_config(&request.identity.project_name)?;
    cfg.simulation = request.config;
    cfg.normalize_simulation_config();
    cfg.validate()?;
    state
        .runtime
        .apply_simulation_live_config(cfg.simulation.clone())
        .await?;
    state
        .project_service
        .save_config(&request.identity.project_name, &cfg)?;
    Ok(Json(serde_json::to_value(cfg.simulation)?))
}

async fn optimization_status(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(serde_json::to_value(
        state.runtime.online_optimization_status().await?,
    )?))
}
async fn optimization_command(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Json(request): Json<ExecutionCommand>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    require_execution(&state, &request.identity).await?;
    match request.action.as_str() {
        "start" => {
            state
                .runtime
                .start_online_optimization(&state.project_service, &request.identity.project_name)
                .await?;
        }
        "stop" => state.runtime.stop_online_optimization().await?,
        "save_best" => {
            let status = state
                .runtime
                .online_optimization_status()
                .await?
                .context("online optimization not running")?;
            if status.phase != crate::online_optimization::OnlineOptimizationPhase::Finished {
                return Err(AppError::status(
                    StatusCode::CONFLICT,
                    "finish optimization before saving best parameters",
                ));
            }
            let params = status
                .best_params
                .into_iter()
                .map(|(key, value)| (key, LogicParamValue::F64(value)))
                .collect();
            state
                .project_service
                .update_logic_params(&request.identity.project_name, params)?;
        }
        _ => {
            return Err(AppError::status(
                StatusCode::BAD_REQUEST,
                "unknown optimization action",
            ));
        }
    }
    optimization_status(State(state)).await
}

#[derive(Deserialize)]
struct HistoryQuery {
    mode: ExecutionMode,
    window_ms: Option<u64>,
}
async fn history(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    state.project_service.load_config(&name)?;
    let path = state
        .project_service
        .project_dir(&name)
        .join("data")
        .join(format!("{}.sqlite", query.mode.label()));
    if !path.exists() {
        return Ok(Json(json!(crate::storage::HistoryWindow {
            sample_limit: crate::storage::HISTORY_SAMPLE_LIMIT,
            ..Default::default()
        })));
    }
    let rows = tokio::task::spawn_blocking(move || {
        SqliteStorage::new(path)?.read_browser_history_window(
            query.window_ms.unwrap_or(180_000).clamp(10_000, 3_600_000),
        )
    })
    .await??;
    Ok(Json(serde_json::to_value(rows)?))
}
async fn history_catalog(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    state.project_service.load_config(&name)?;
    let path = state
        .project_service
        .project_dir(&name)
        .join("data")
        .join(format!("{}.sqlite", query.mode.label()));
    if !path.exists() {
        return Ok(Json(json!([])));
    }
    let rows = tokio::task::spawn_blocking(move || SqliteStorage::new(path)?.read_series_catalog())
        .await??;
    Ok(Json(serde_json::to_value(rows)?))
}
async fn logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Value>, AppError> {
    state.project_service.load_config(&name)?;
    let path = state
        .project_service
        .project_dir(&name)
        .join("data")
        .join(format!("{}.log", query.mode.label()));
    if !path.exists() {
        return Ok(Json(json!([])));
    }
    let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        let start = length.saturating_sub(256_000);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(256_000).read_to_end(&mut bytes)?;
        let tail = String::from_utf8_lossy(&bytes);
        let lines = tail
            .lines()
            .skip(usize::from(start > 0))
            .collect::<Vec<_>>();
        Ok(lines
            .into_iter()
            .rev()
            .filter_map(|line| serde_json::from_str(line).ok())
            .take(200)
            .collect())
    })
    .await??;
    Ok(Json(json!(rows)))
}

use anyhow::Context;

#[derive(Deserialize)]
struct OnlineConfigUpdate {
    revision: String,
    config: crate::project_config::OnlineOptimizationConfig,
}
async fn save_online_config(
    State(state): State<AppState>,
    Extension(engineering): Extension<EngineeringService>,
    Path(name): Path<String>,
    Json(request): Json<OnlineConfigUpdate>,
) -> Result<Json<Value>, AppError> {
    let _mutation = engineering.mutations.lock().await;
    let active_project = state.runtime.active_project().await;
    if engineering.project_busy(&name)
        || active_project.as_deref() == Some(&name)
            && state
                .runtime
                .online_optimization_status()
                .await?
                .is_some_and(|status| {
                    status.phase == crate::online_optimization::OnlineOptimizationPhase::Running
                })
    {
        return Err(AppError::status(
            StatusCode::CONFLICT,
            "optimization configuration is locked during a running optimization or engineering job",
        ));
    }
    if request.revision != config_revision(&state.project_service, &name)? {
        return Err(AppError::status(
            StatusCode::PRECONDITION_FAILED,
            "project changed; reload before saving objective configuration",
        ));
    }
    let mut cfg = state.project_service.load_config(&name)?;
    cfg.online_optimization = request.config;
    cfg.sync_active_online_params();
    // Validate user safety limits before normalization can clamp them.
    cfg.validate()?;
    state.project_service.save_config(&name, &cfg)?;
    Ok(Json(
        json!({"config": state.project_service.load_config(&name)?.online_optimization, "revision": config_revision(&state.project_service, &name)?}),
    ))
}
