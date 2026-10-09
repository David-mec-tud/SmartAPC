//! Application-level engineering work, independent of UI windows and control tasks.
//! Jobs retain project/configuration identity and explicit terminal results.
use anyhow::{Context, Result, bail};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    communication::{ProtocolAdapter, opcua::OpcUaClientAdapter},
    control_function_tests::{
        SweepSelection, default_sweep_dimensions, run_control_function_tests,
    },
    kalman_tuning::{import_csv_bytes, kalman_parameter_values, run_observer, tuning_store},
    parameter_optimization::{OptimizationEvent, run_parameter_optimization},
    project::ProjectService,
    project_config::{LogicKind, ProjectConfig, WatchdogConfig},
    runtime::RuntimeManager,
    strategy_service,
    types::SignalValue,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub project_name: String,
    pub config_revision: String,
    pub kind: String,
    pub phase: JobPhase,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub cancellable: bool,
    pub progress: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl JobSnapshot {
    /// Lists carry status only. Replay arrays and optimizer curves are fetched
    /// explicitly by job ID, instead of being retransmitted on every list poll.
    fn brief(&self) -> Self {
        let progress = self
            .progress
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "curves" | "convergence_history"))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<serde_json::Map<_, _>>()
            })
            .unwrap_or_default();
        Self {
            id: self.id.clone(),
            project_name: self.project_name.clone(),
            config_revision: self.config_revision.clone(),
            kind: self.kind.clone(),
            phase: self.phase.clone(),
            started_at: self.started_at.clone(),
            finished_at: self.finished_at.clone(),
            cancellable: self.cancellable,
            progress: Value::Object(progress),
            result: None,
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobRequest {
    StrategyBuild,
    ControlTests {
        selections: Vec<SweepSelection>,
    },
    KalmanReplay {
        file_name: String,
        csv: String,
        parameters: HashMap<String, f64>,
    },
    OfflineOptimization,
    IoRead,
    IoWrite {
        values: HashMap<String, SignalValue>,
    },
    WatchdogTest,
    ControllerSignalsTest,
}

impl JobRequest {
    fn kind(&self) -> &'static str {
        match self {
            Self::StrategyBuild => "strategy_build",
            Self::ControlTests { .. } => "control_tests",
            Self::KalmanReplay { .. } => "kalman_replay",
            Self::OfflineOptimization => "offline_optimization",
            Self::IoRead => "io_read",
            Self::IoWrite { .. } => "io_write",
            Self::WatchdogTest => "watchdog_test",
            Self::ControllerSignalsTest => "controller_signals_test",
        }
    }
}

struct JobRecord {
    snapshot: JobSnapshot,
    cancel: Arc<AtomicBool>,
    // A persistence failure must not let a later job evict the only full report.
    persisted: bool,
}

#[derive(Clone, Default)]
pub struct EngineeringService {
    /// Serialize HTTP configuration/lifecycle mutations across browser clients.
    /// Blocking work runs after releasing this lock, protected by job ownership.
    pub mutations: Arc<Mutex<()>>,
    jobs: Arc<RwLock<HashMap<String, JobRecord>>>,
}

pub fn config_revision(service: &ProjectService, name: &str) -> Result<String> {
    // Hash persisted bytes, not HashMap iteration order or a client's draft.
    Ok(content_revision(&fs::read(service.config_path(name))?))
}

/// An optimistic revision for a saved file, including edits from external editors.
pub fn content_revision(bytes: &[u8]) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    format!("\"{:016x}\"", hash.finish())
}

impl EngineeringService {
    pub fn project_busy(&self, name: &str) -> bool {
        self.jobs.read().values().any(|job| {
            job.snapshot.project_name == name && matches!(job.snapshot.phase, JobPhase::Running)
        })
    }

    pub async fn require_editable(&self, runtime: &RuntimeManager, name: &str) -> Result<()> {
        if runtime.active_project().await.as_deref() == Some(name) {
            bail!("configuration is locked while this project is executing; stop execution first");
        }
        if self.project_busy(name) {
            bail!("configuration is locked while an engineering job is running");
        }
        Ok(())
    }

    pub fn list(&self, service: &ProjectService, name: &str) -> Result<Vec<JobSnapshot>> {
        let mut jobs = self
            .jobs
            .read()
            .values()
            .filter(|job| job.snapshot.project_name == name)
            .map(|job| job.snapshot.brief())
            .collect::<Vec<_>>();
        let dir = service.project_dir(name).join("data/jobs");
        if dir.exists() {
            let mut paths = fs::read_dir(dir)?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().is_some_and(|ext| ext == "json")
                        && !path.to_string_lossy().ends_with(".summary.json")
                })
                .map(|path| {
                    (
                        fs::metadata(&path)
                            .and_then(|metadata| metadata.modified())
                            .ok(),
                        path,
                    )
                })
                .collect::<Vec<_>>();
            paths.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, path) in paths.into_iter().take(64) {
                let summary = path.with_extension("summary.json");
                let cached = summary.is_file();
                let snapshot: JobSnapshot =
                    serde_json::from_slice(&fs::read(if cached { &summary } else { &path })?)?;
                let snapshot = snapshot.brief();
                if !cached {
                    // Upgrade older persisted job reports once, without changing
                    // their full report or discarding any historical results.
                    if let Err(err) = fs::write(&summary, serde_json::to_vec(&snapshot)?) {
                        tracing::warn!(error = %err, "job summary persistence failed");
                    }
                }
                if !jobs.iter().any(|job| job.id == snapshot.id) {
                    jobs.push(snapshot);
                }
            }
        }
        jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        jobs.truncate(64);
        Ok(jobs)
    }

    pub fn get(&self, service: &ProjectService, name: &str, id: &str) -> Result<JobSnapshot> {
        Uuid::parse_str(id).context("invalid job ID")?;
        if let Some(job) = self.jobs.read().get(id) {
            if job.snapshot.project_name != name {
                bail!("job project does not match");
            }
            return Ok(job.snapshot.clone());
        }
        let path = service
            .project_dir(name)
            .join("data/jobs")
            .join(format!("{id}.json"));
        serde_json::from_slice(&fs::read(path).context("job not found")?)
            .context("invalid job result")
    }

    pub fn cancel(&self, name: &str, id: &str) -> Result<()> {
        let jobs = self.jobs.read();
        let job = jobs.get(id).context("running job not found")?;
        if job.snapshot.project_name != name {
            bail!("job project does not match");
        }
        if !job.snapshot.cancellable {
            bail!("this job does not support cancellation");
        }
        if !matches!(job.snapshot.phase, JobPhase::Running) {
            bail!("this job has already finished");
        }
        job.cancel.store(true, Ordering::Release);
        Ok(())
    }

    /// Caller holds `mutations` until this record is registered. The worker owns
    /// cancellation; navigation, transport reconnect and tab closure have no role.
    pub fn start(
        &self,
        service: ProjectService,
        name: String,
        request: JobRequest,
    ) -> Result<JobSnapshot> {
        if self.project_busy(&name) {
            bail!("an engineering job is already running for this project");
        }
        if self
            .jobs
            .read()
            .values()
            .filter(|job| matches!(job.snapshot.phase, JobPhase::Running))
            .count()
            >= 2
        {
            bail!("two engineering jobs are already running; wait for one to finish");
        }
        let cfg = service.load_config(&name)?;
        let id = Uuid::new_v4().to_string();
        let snapshot = JobSnapshot {
            id: id.clone(),
            project_name: name.clone(),
            config_revision: config_revision(&service, &name)?,
            kind: request.kind().into(),
            phase: JobPhase::Running,
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            cancellable: matches!(request, JobRequest::OfflineOptimization),
            progress: json!({"status": "Starting"}),
            result: None,
            error: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.write().insert(
            id.clone(),
            JobRecord {
                snapshot: snapshot.clone(),
                cancel: cancel.clone(),
                persisted: false,
            },
        );
        let owner = self.clone();
        tokio::spawn(async move {
            let result = match request {
                JobRequest::IoRead => test_io_read(&cfg).await,
                JobRequest::IoWrite { values } => test_io_write(&cfg, values).await,
                JobRequest::WatchdogTest => test_watchdog(&cfg).await,
                JobRequest::ControllerSignalsTest => test_controller_signals(&cfg).await,
                blocking_request => {
                    let worker_service = service.clone();
                    let worker_name = name.clone();
                    let worker_owner = owner.clone();
                    let worker_id = id.clone();
                    let worker_cancel = cancel.clone();
                    match tokio::task::spawn_blocking(move || {
                        run_blocking_job(
                            &worker_service,
                            &worker_name,
                            cfg,
                            blocking_request,
                            worker_cancel,
                            worker_owner,
                            &worker_id,
                        )
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(err) => Err(anyhow::anyhow!("engineering worker failed: {err}")),
                    }
                }
            };
            let terminal = {
                let mut jobs = owner.jobs.write();
                let job = jobs.get_mut(&id).expect("registered job");
                job.snapshot.finished_at = Some(chrono::Utc::now().to_rfc3339());
                match result {
                    Ok(result) => {
                        job.snapshot.phase = if result.get("success") == Some(&Value::Bool(false))
                            || result.get("compatible") == Some(&Value::Bool(false))
                        {
                            job.snapshot.error = result
                                .get("message")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            JobPhase::Failed
                        } else if cancel.load(Ordering::Acquire) {
                            JobPhase::Cancelled
                        } else {
                            JobPhase::Succeeded
                        };
                        job.snapshot.result = Some(result);
                    }
                    Err(err) => {
                        job.snapshot.phase = JobPhase::Failed;
                        job.snapshot.error = Some(format!("{err:#}"));
                    }
                }
                job.snapshot.clone()
            };
            let dir = service.project_dir(&name).join("data/jobs");
            let persisted = (|| -> Result<()> {
                fs::create_dir_all(&dir)?;
                fs::write(
                    dir.join(format!("{id}.json")),
                    serde_json::to_vec_pretty(&terminal)?,
                )?;
                fs::write(
                    dir.join(format!("{id}.summary.json")),
                    serde_json::to_vec(&terminal.brief())?,
                )?;
                Ok(())
            })();
            if let Err(err) = persisted {
                tracing::error!(job = %id, error = %err, "job result persistence failed");
            } else {
                // Retain running jobs and a small terminal cache. Full reports
                // remain readable by ID from disk after eviction or restart.
                let mut jobs = owner.jobs.write();
                jobs.get_mut(&id).expect("registered job").persisted = true;
                let mut terminal_ids = jobs
                    .values()
                    .filter(|job| job.persisted && !matches!(job.snapshot.phase, JobPhase::Running))
                    .map(|job| (job.snapshot.started_at.clone(), job.snapshot.id.clone()))
                    .collect::<Vec<_>>();
                terminal_ids.sort_by(|a, b| b.0.cmp(&a.0));
                for (_, old_id) in terminal_ids.into_iter().skip(8) {
                    jobs.remove(&old_id);
                }
            }
        });
        Ok(snapshot)
    }

    fn progress(&self, id: &str, progress: Value) {
        if let Some(job) = self.jobs.write().get_mut(id) {
            job.snapshot.progress = progress;
        }
    }
}

fn run_blocking_job(
    service: &ProjectService,
    name: &str,
    cfg: ProjectConfig,
    request: JobRequest,
    cancel: Arc<AtomicBool>,
    owner: EngineeringService,
    id: &str,
) -> Result<Value> {
    match request {
        JobRequest::StrategyBuild => {
            owner.progress(
                id,
                json!({"status": "Compiling and checking strategy contract"}),
            );
            Ok(serde_json::to_value(strategy_service::build_strategy(
                service, name,
            )?)?)
        }
        JobRequest::ControlTests { selections } => {
            owner.progress(
                id,
                json!({"status": "Evaluating sweeps and output contracts"}),
            );
            Ok(serde_json::to_value(run_control_function_tests(
                &cfg,
                &service.project_dir(name),
                &selections,
            )?)?)
        }
        JobRequest::KalmanReplay {
            file_name,
            csv,
            parameters,
        } => {
            let known = kalman_parameter_values(&cfg)?;
            if known.len() != parameters.len()
                || parameters.keys().any(|key| !known.contains_key(key))
            {
                bail!("observer parameters must match the active observer schema");
            }
            let dataset = import_csv_bytes(&file_name, csv.as_bytes(), &cfg.variables)?;
            let model = cfg
                .model
                .as_ref()
                .and_then(|model| model.linear_state_space())
                .context("linear model is required")?;
            owner.progress(
                id,
                json!({"status": "Replaying observer", "rows": dataset.row_count()}),
            );
            let result = run_observer(
                name,
                &dataset,
                model,
                &cfg.variables,
                cfg.logic.cycle_ms,
                &parameters,
            )?;
            tuning_store().put(result.clone());
            Ok(serde_json::to_value(result)?)
        }
        JobRequest::OfflineOptimization => {
            let (tx, rx) = mpsc::channel();
            let progress_owner = owner.clone();
            let progress_id = id.to_string();
            // A short-lived forwarding thread reports progress while the algorithm
            // retains its existing synchronous, cancellable implementation.
            let reporter = std::thread::spawn(move || {
                while let Ok(event) = rx.recv() {
                    if let OptimizationEvent::Progress(progress) = event {
                        progress_owner.progress(
                            &progress_id,
                            serde_json::to_value(progress).unwrap_or(Value::Null),
                        );
                    }
                }
            });
            let result = run_parameter_optimization(service, name, cancel, Some(tx));
            reporter
                .join()
                .map_err(|_| anyhow::anyhow!("optimization progress worker failed"))?;
            Ok(serde_json::to_value(result?)?)
        }
        _ => bail!("invalid blocking job"),
    }
}

async fn test_io_read(cfg: &ProjectConfig) -> Result<Value> {
    let adapter = OpcUaClientAdapter::new(cfg.communication.endpoint_url.clone());
    adapter.connect().await?;
    let mut results = Vec::new();
    // Include output/readback and external band nodes as in the native read test.
    for point in cfg.io.inputs.iter().chain(&cfg.io.outputs) {
        let result = adapter.read(&point.address).await;
        results.push(match result {
            Ok(value) => json!({"signal": point.name, "address": point.address, "value": value}),
            Err(err) => {
                json!({"signal": point.name, "address": point.address, "error": err.to_string()})
            }
        });
    }
    Ok(json!({"readings": results}))
}

async fn test_io_write(cfg: &ProjectConfig, values: HashMap<String, SignalValue>) -> Result<Value> {
    // Validate the entire request before sending any value. Test writes are limited
    // to configured MV writes and CV setpoints, never arbitrary browser Node IDs.
    let mut writes = Vec::new();
    for (signal, value) in values {
        let point = cfg
            .io
            .inputs
            .iter()
            .chain(&cfg.io.outputs)
            .find(|point| point.name == signal)
            .context("unknown I/O test signal")?;
        if !(signal.starts_with("mv.") && signal.ends_with(".write")
            || signal.starts_with("cv.") && signal.ends_with(".setpoint"))
        {
            bail!("this signal cannot be written in an I/O test");
        }
        match (&value, point.signal_type) {
            (SignalValue::F64(value), crate::types::SignalType::F64) if value.is_finite() => {}
            (SignalValue::Bool(_), crate::types::SignalType::Bool) => {}
            _ => bail!("I/O test value type mismatch"),
        }
        writes.push((point.address.clone(), value));
    }
    let adapter = OpcUaClientAdapter::new(cfg.communication.endpoint_url.clone());
    adapter.connect().await?;
    let count = writes.len();
    for (address, value) in writes {
        adapter.write(&address, value).await?;
    }
    Ok(json!({"writes": count}))
}

async fn test_watchdog(cfg: &ProjectConfig) -> Result<Value> {
    match &cfg.watchdog {
        WatchdogConfig::None => {
            Ok(json!({"status": "disabled", "message": "No watchdog test is required"}))
        }
        WatchdogConfig::CounterMirror {
            read_node,
            write_node,
            stale_timeout_ms,
        } => {
            let adapter = OpcUaClientAdapter::new(cfg.communication.endpoint_url.clone());
            adapter.connect().await?;
            let counter = adapter
                .read(read_node)
                .await?
                .as_f64()
                .context("watchdog counter must be numeric")?;
            adapter.write(write_node, SignalValue::F64(counter)).await?;
            Ok(
                json!({"status": "passed", "counter": counter, "communication_cycle_ms": cfg.communication.cycle_ms, "stale_timeout_ms": stale_timeout_ms}),
            )
        }
    }
}

/// Test the saved handshake contract without claiming readiness or ownership.
/// Outputs are written false and read back; the external enable is only read.
async fn test_controller_signals(cfg: &ProjectConfig) -> Result<Value> {
    cfg.validate()?;
    let adapter = OpcUaClientAdapter::new(cfg.communication.endpoint_url.clone());
    crate::runtime::seed_protocol_nodes(&adapter, cfg)?;
    adapter.connect().await?;
    let mut readings = Vec::new();
    for (signal, address) in cfg.deployment.signal_nodes() {
        let result: Result<SignalValue> = async {
            let initial = adapter.read(address).await?;
            if !matches!(initial, SignalValue::Bool(_)) {
                bail!("{signal} must be an OPC UA Boolean");
            }
            if signal != "ExternalControllerEnable" {
                adapter.write(address, SignalValue::Bool(false)).await?;
                let readback = adapter.read(address).await?;
                if readback != SignalValue::Bool(false) {
                    bail!("{signal} false write was not confirmed by readback");
                }
                Ok(readback)
            } else {
                Ok(initial)
            }
        }
        .await;
        readings.push(match result {
            Ok(value) => json!({"signal": signal, "address": address, "value": value}),
            Err(err) => json!({"signal": signal, "address": address, "error": format!("{err:#}")}),
        });
    }
    Ok(
        json!({"readings": readings, "status": if cfg.deployment.signal_nodes().is_empty() { "disabled" } else { "tested" }}),
    )
}

/// Readiness concerns saved configuration; it does not contact or write hardware.
pub fn project_readiness(service: &ProjectService, name: &str, cfg: &ProjectConfig) -> Value {
    let mut issues = Vec::new();
    if let Err(err) = cfg.validate() {
        issues.push(json!({"page": "/configuration/communication", "message": err.to_string()}));
    }
    if !cfg.variables.has_any() {
        issues.push(json!({"page": "/configuration/io", "message": "Define the MV/CV/DV catalog and I/O contract"}));
    }
    let compatible = if cfg.logic.kind == LogicKind::LinearMpc {
        cfg.linear_mpc_is_configured_for(&cfg.io)
    } else {
        cfg.logic.is_configured_for(&cfg.io)
    };
    if !compatible {
        issues.push(json!({"page": "/configuration/strategy", "message": "Selected strategy is not compatible with the current I/O contract"}));
    }
    if cfg.logic.kind == LogicKind::CustomRust && compatible {
        if let Err(err) = crate::runtime::build_logic(cfg, &cfg.io, &service.project_dir(name)) {
            issues.push(json!({"page": "/configuration/strategy", "message": err.to_string()}));
        }
    }
    json!({"ready": issues.is_empty(), "issues": issues, "strategy": cfg.logic.kind,
        "model_requirement": if cfg.logic.kind == LogicKind::LinearMpc { "required" } else { "optional" },
        "optional": ["Simulation", "Kalman tuning", "Offline optimization"],
        "hardware_verified": false,
        "windows_service_support": "Command generation only. Native Windows service lifecycle is not implemented or verified."})
}

pub fn engineering_catalog(cfg: &ProjectConfig) -> Result<Value> {
    let mut active = cfg.clone();
    active.sync_active_online_params();
    Ok(
        json!({"sweeps": default_sweep_dimensions(&active), "parameters": active.logic.params,
        "kalman_parameters": kalman_parameter_values(&active).ok(),
        "online_optimization_parameters": active.online_optimization_param_defaults(),
        "cv_safety_limits": active.predefined_cv_limits(),
        "simulation_available": active.validate_for_simulation().is_ok(),
        "simulation_issue": active.validate_for_simulation().err().map(|err| err.to_string()),
        "offline_optimization_available": active.logic.kind != LogicKind::CustomRust && active.validate_for_simulation().is_ok()}),
    )
}
