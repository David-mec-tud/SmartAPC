use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::json;
use tokio::{
    sync::{Mutex, Notify, oneshot},
    task::JoinHandle,
};

use crate::{
    communication::{HealthStatus, ProtocolAdapter, opcua::OpcUaClientAdapter},
    controller_signals::{ControllerSignals, ControllerStatus},
    linear_mpc::LinearMpcLogic,
    logic::{ControlLogic, CustomRustLogic, LogicUserLog, PControllerLogic},
    model::ModelModule,
    online_optimization::{
        OnlineOptimizationSession, OnlineOptimizationStatus, OnlineOptimizationTick,
    },
    project::ProjectService,
    project_config::{
        IoConfig, LogicKind, LogicParamValue, ModelConfig, ProjectConfig, SimulationConfig,
        WatchdogConfig,
    },
    simulation::{SimulationCore, SimulationSnapshot},
    storage::{CycleMetric, SeriesCatalogEntry, SignalSample, SqliteStorage},
    types::{RuntimeState, SignalType, SignalValue},
    watchdog::{
        CounterMirrorWatchdog, NoneWatchdog, WatchdogModule, WatchdogStatus,
        stale_timeout_or_default,
    },
};

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeMetrics {
    pub total_cycles: u64,
    pub deadline_misses: u64,
    pub last_duration_us: u64,
    pub last_jitter_us: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub controller: ControllerStatus,
    pub execution_id: String,
    pub project_name: String,
    pub state: RuntimeState,
    pub connection_health: HealthStatus,
    pub watchdog_status: WatchdogStatus,
    pub metrics: RuntimeMetrics,
    pub last_inputs: HashMap<String, SignalValue>,
    pub last_outputs: HashMap<String, SignalValue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "mode", content = "snapshot", rename_all = "snake_case")]
pub enum ExecutionSnapshot {
    Runtime(RuntimeSnapshot),
    Simulation(SimulationSnapshot),
}

struct RuntimeCore {
    controller: Arc<ControllerSignals>,
    execution_id: String,
    config: ProjectConfig,
    protocol: Arc<dyn ProtocolAdapter>,
    watchdog: Box<dyn WatchdogModule>,
    logic: Box<dyn ControlLogic>,
    model: Option<Box<dyn ModelModule>>,
    storage: SqliteStorage,
    log_file: File,
    state: RuntimeState,
    watchdog_status: WatchdogStatus,
    fault_started_at: Option<Instant>,
    input_fault: bool,
    last_inputs: HashMap<String, SignalValue>,
    last_outputs: HashMap<String, SignalValue>,
    total_cycles: u64,
    deadline_misses: u64,
    last_duration_us: u64,
    last_jitter_us: i64,
    last_cycle_start: Option<Instant>,
    last_communication_cycle: Option<Instant>,
    last_io_cycle: Option<Instant>,
    last_controller_cycle: Option<Instant>,
    online_optimization: Option<OnlineOptimizationSession>,
    online_optimization_stop: Arc<AtomicBool>,
}

impl RuntimeCore {
    async fn new(
        config: ProjectConfig,
        protocol: Arc<dyn ProtocolAdapter>,
        mut watchdog: Box<dyn WatchdogModule>,
        mut logic: Box<dyn ControlLogic>,
        model: Option<Box<dyn ModelModule>>,
        storage: SqliteStorage,
        log_file: File,
    ) -> Result<Self> {
        protocol.connect().await?;
        let controller = Arc::new(ControllerSignals::new(
            config.deployment.clone(),
            Arc::clone(&protocol),
            config.communication.cycle_ms,
        ));
        // Clear stale device flags before watchdog/strategy initialization.
        {
            let _exchange = controller.exchange.lock().await;
            controller.publish().await?;
        }
        watchdog.init(protocol.as_ref()).await?;
        logic.init(
            config.logic.initial_values.clone(),
            config.logic.params.clone(),
        );

        Ok(Self {
            controller,
            execution_id: uuid::Uuid::new_v4().to_string(),
            config,
            protocol,
            watchdog,
            logic,
            model,
            storage,
            log_file,
            state: RuntimeState::Running,
            watchdog_status: WatchdogStatus::Ok,
            fault_started_at: None,
            input_fault: false,
            last_inputs: HashMap::new(),
            last_outputs: HashMap::new(),
            total_cycles: 0,
            deadline_misses: 0,
            last_duration_us: 0,
            last_jitter_us: 0,
            last_cycle_start: None,
            last_communication_cycle: None,
            last_io_cycle: None,
            last_controller_cycle: None,
            online_optimization: None,
            online_optimization_stop: Arc::new(AtomicBool::new(false)),
        })
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            controller: self.controller.snapshot(),
            execution_id: self.execution_id.clone(),
            project_name: self.config.name.clone(),
            state: self.state,
            connection_health: self.protocol.health(),
            watchdog_status: self.watchdog_status.clone(),
            metrics: RuntimeMetrics {
                total_cycles: self.total_cycles,
                deadline_misses: self.deadline_misses,
                last_duration_us: self.last_duration_us,
                last_jitter_us: self.last_jitter_us,
            },
            last_inputs: self.last_inputs.clone(),
            last_outputs: self.last_outputs.clone(),
        }
    }

    fn current_params(&self) -> HashMap<String, LogicParamValue> {
        self.logic.current_params()
    }

    fn set_params(&mut self, updates: HashMap<String, LogicParamValue>) -> Result<()> {
        if self.online_optimization.as_ref().is_some_and(|session| {
            session.status().phase != crate::online_optimization::OnlineOptimizationPhase::Finished
        }) {
            bail!("manual parameter updates are locked during online optimization");
        }
        let current = self.logic.current_params();
        for (key, value) in &updates {
            let existing = current
                .get(key)
                .ok_or_else(|| anyhow!("unknown online parameter '{}'", key))?;
            if !existing.matches_type(value.param_type()) {
                bail!(
                    "online parameter '{}' type mismatch (expected {:?})",
                    key,
                    existing.param_type()
                );
            }
        }

        self.logic.set_params(updates.clone());
        if let Err(err) = self.append_log_record(json!({
            "timestamp_ms": now_ms(),
            "event": "online_params_update",
            "state": self.state,
            "updates": updates,
            "params": self.logic.current_params(),
        })) {
            tracing::warn!(error = %err, "failed to write parameter update log");
        }
        Ok(())
    }

    fn start_online_optimization(
        &mut self,
        config: crate::project_config::OnlineOptimizationConfig,
    ) -> Result<()> {
        if self.online_optimization.is_some() {
            bail!("online optimization is already active");
        }
        self.online_optimization_stop
            .store(false, Ordering::Release);
        let (session, events) = OnlineOptimizationSession::new(
            config,
            &self.logic.current_params(),
            self.config.logic.cycle_ms,
            Arc::clone(&self.online_optimization_stop),
        )?;
        self.online_optimization = Some(session);
        self.append_online_optimization_events(events)
    }

    fn online_optimization_status(&self) -> Option<OnlineOptimizationStatus> {
        self.online_optimization
            .as_ref()
            .map(OnlineOptimizationSession::status)
    }

    fn apply_online_optimization_tick(&mut self, tick: OnlineOptimizationTick) -> Result<()> {
        if let Some(updates) = tick.updates {
            self.logic.set_params(updates);
        }
        self.append_online_optimization_events(tick.events)
    }

    fn append_online_optimization_events(&mut self, events: Vec<serde_json::Value>) -> Result<()> {
        for mut event in events {
            if let Some(object) = event.as_object_mut() {
                object.insert("timestamp_ms".to_string(), json!(now_ms()));
                object.insert("state".to_string(), json!(self.state));
            }
            self.append_log_record(event)?;
        }
        Ok(())
    }

    fn read_plot_samples(&self, limit: usize) -> Result<Vec<SignalSample>> {
        self.storage.read_recent_samples(limit)
    }

    fn read_plot_samples_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        self.storage.read_latest_samples_window(window_ms)
    }

    fn read_plot_catalog(&self) -> Result<Vec<SeriesCatalogEntry>> {
        self.storage.read_series_catalog()
    }

    async fn simulate_step(
        &mut self,
        inputs: HashMap<String, SignalValue>,
    ) -> Result<HashMap<String, SignalValue>> {
        let mut outputs = self.logic.step(&inputs, RuntimeState::Running)?;
        apply_output_limits(&self.config, &mut outputs);
        if let Some(model) = self.model.as_mut() {
            outputs = model.simulate_step(&outputs)?;
        }
        Ok(outputs)
    }

    async fn tick_once(&mut self) -> Result<()> {
        let result = self.tick_inner().await;
        if let Err(err) = &result {
            self.state = self.compute_next_state(false);
            if let Err(clear_error) = self
                .controller
                .revoke(format!("Runtime error: {err:#}"))
                .await
            {
                tracing::error!(error = %clear_error, "failed clearing controller flags");
            }
            // Keep the fault and its effect machine-readable even after a failed cycle.
            let _ = self.append_log_record(json!({"timestamp_ms": now_ms(), "event": "controller_fault", "error": format!("{err:#}"), "controller": self.controller.snapshot()}));
        }
        result
    }

    async fn tick_inner(&mut self) -> Result<()> {
        if self.controller.is_stopped() {
            return Ok(());
        }
        let cycle_start = Instant::now();
        let communication_due = cycle_due(
            self.last_communication_cycle,
            cycle_start,
            self.config.communication.cycle_ms,
        );
        let io_due = cycle_due(self.last_io_cycle, cycle_start, self.config.io.cycle_ms);
        let controller_due = cycle_due(
            self.last_controller_cycle,
            cycle_start,
            self.config.logic.cycle_ms,
        );
        let period = Duration::from_millis(self.config.logic.cycle_ms);
        let scheduled_start = self
            .last_controller_cycle
            .map(|last| last + period)
            .unwrap_or(cycle_start);
        let suggestion_deadline = scheduled_start + period;
        if controller_due {
            self.controller.arm(suggestion_deadline);
        }
        let generation = self.controller.generation();

        let input_ok = if io_due {
            match self.read_inputs().await {
                Ok(inputs) => {
                    self.storage.store_signal_map("input", &inputs)?;
                    self.last_inputs = inputs;
                    self.input_fault = false;
                    true
                }
                Err(err) => {
                    self.input_fault = true;
                    self.controller
                        .revoke(format!("Input error: {err:#}"))
                        .await?;
                    false
                }
            }
        } else {
            true
        };
        if io_due {
            if let Err(err) = self.controller.read_enable().await {
                self.controller.set_enabled(false, true)?;
                return Err(err);
            }
        }
        if communication_due {
            self.watchdog_status = match tokio::time::timeout(
                Duration::from_millis(self.config.communication.cycle_ms),
                self.watchdog.tick(self.protocol.as_ref()),
            )
            .await
            {
                Ok(Ok(status)) => status,
                Ok(Err(err)) => WatchdogStatus::Fault {
                    reason: format!("watchdog tick failed: {err}"),
                },
                Err(err) => WatchdogStatus::Fault {
                    reason: format!("watchdog tick timed out: {err}"),
                },
            };
            self.last_communication_cycle = Some(cycle_start);
        }
        // An I/O failure takes effect immediately, including between watchdog ticks.
        let healthy = input_ok
            && !self.input_fault
            && self.protocol.health() == HealthStatus::Healthy
            && !matches!(&self.watchdog_status, WatchdogStatus::Fault { .. });
        self.state = self.compute_next_state(healthy);
        if !healthy {
            self.controller
                .revoke("Communication or watchdog fault")
                .await?;
        }

        let mut user_logs = Vec::new();
        if controller_due {
            if let Some(mut session) = self.online_optimization.take() {
                let tick = session.pre_cycle(self.state, &self.last_inputs);
                self.apply_online_optimization_tick(tick)?;
                self.online_optimization = Some(session);
            }
            if self.state == RuntimeState::Running {
                // A custom synchronous controller may be slow. Keep the async
                // deadline monitor able to revoke readiness while it calculates.
                let mut logic = std::mem::replace(
                    &mut self.logic,
                    Box::new(PControllerLogic::new(Vec::new(), HashMap::new())),
                );
                let status = self.controller.snapshot();
                logic.set_shadow_mode(!status.enabled || !status.is_ready);
                let inputs = self.last_inputs.clone();
                let state = self.state;
                let (returned_logic, result) = tokio::task::spawn_blocking(move || {
                    let result = logic.step(&inputs, state);
                    (logic, result)
                })
                .await
                .context("controller worker failed")?;
                self.logic = returned_logic;
                let mut outputs = result?;
                user_logs = self.logic.drain_user_logs();
                // Invalid values must never become an actuator command or readiness.
                for point in &self.config.io.outputs {
                    if let Some(value) = outputs.get(&point.name) {
                        match (value, point.signal_type) {
                            (SignalValue::F64(v), SignalType::F64) if v.is_finite() => {}
                            (SignalValue::Bool(_), SignalType::Bool) => {}
                            _ => bail!("invalid controller output '{}'", point.name),
                        }
                    }
                }
                let successful = self.logic.suggestion_succeeded()
                    && self
                        .config
                        .io
                        .outputs
                        .iter()
                        .any(|point| outputs.contains_key(&point.name));
                apply_output_limits(&self.config, &mut outputs);
                self.last_outputs = outputs;
                if Instant::now() < suggestion_deadline {
                    self.controller.suggestion(
                        generation,
                        self.logic.observer_converged(),
                        successful,
                        scheduled_start + period * 2,
                    );
                } else {
                    self.controller
                        .revoke("Control suggestion deadline missed")
                        .await?;
                }
            }
            self.store_online_param_samples()?;
            self.store_user_log_samples(&user_logs)?;
            if let Some(mut session) = self.online_optimization.take() {
                let tick = session.observe(&self.last_inputs, &self.last_outputs);
                self.apply_online_optimization_tick(tick)?;
                self.online_optimization = Some(session);
            }
            self.total_cycles += 1;
            // Advance scheduled slots rather than accumulating scheduler jitter.
            self.last_controller_cycle = Some(if cycle_start >= suggestion_deadline {
                cycle_start
            } else {
                scheduled_start
            });
        }

        if io_due {
            let controller = Arc::clone(&self.controller);
            let _exchange = controller.exchange.lock().await;
            let status = controller.snapshot();
            if self.state == RuntimeState::Running
                && status.enabled
                && status.is_ready
                && !self.last_outputs.is_empty()
            {
                self.write_outputs(&self.last_outputs).await?;
                self.logic.outputs_applied(&self.last_outputs);
                self.storage
                    .store_signal_map("output", &self.last_outputs)?;
                controller.mark_control(true);
            } else {
                controller.mark_control(false);
            }
            controller.publish().await?;
            self.last_io_cycle = Some(cycle_start);
        }
        if controller_due {
            if !io_due {
                let _exchange = self.controller.exchange.lock().await;
                self.controller.publish().await?;
            }
            let missed = Instant::now() >= suggestion_deadline;
            self.update_metrics(cycle_start, cycle_start.elapsed(), missed)?;
            if missed {
                self.controller
                    .revoke("Control cycle deadline missed")
                    .await?;
            }
            self.append_cycle_log(
                &self.last_inputs.clone(),
                Some(&self.last_outputs.clone()),
                &user_logs,
            )?;
        }
        Ok(())
    }

    async fn read_inputs(&self) -> Result<HashMap<String, SignalValue>> {
        let mut inputs = HashMap::new();
        for point in &self.config.io.inputs {
            let value = tokio::time::timeout(
                Duration::from_millis(self.config.communication.cycle_ms),
                self.protocol.read(&point.address),
            )
            .await
            .context("input read timed out")?
            .with_context(|| format!("failed reading input '{}'", point.name))?;
            inputs.insert(point.name.clone(), value);
        }
        Ok(inputs)
    }

    fn compute_next_state(&mut self, healthy: bool) -> RuntimeState {
        if healthy {
            self.fault_started_at = None;
            return RuntimeState::Running;
        }

        let now = Instant::now();
        let fault_started = self.fault_started_at.get_or_insert(now);
        let elapsed = now.saturating_duration_since(*fault_started);

        if elapsed < Duration::from_millis(self.config.alarms.alarm_timeout_ms) {
            RuntimeState::Hold
        } else if elapsed < Duration::from_millis(self.config.alarms.off_timeout_ms) {
            RuntimeState::Alarm
        } else {
            RuntimeState::Off
        }
    }

    async fn write_outputs(&self, outputs: &HashMap<String, SignalValue>) -> Result<()> {
        for point in &self.config.io.outputs {
            if !self.controller.snapshot().is_ready {
                bail!("controller readiness expired before actuator write");
            }
            if let Some(value) = outputs.get(&point.name) {
                tokio::time::timeout(
                    Duration::from_millis(self.config.communication.cycle_ms),
                    self.protocol.write(&point.address, value.clone()),
                )
                .await
                .context("output write timed out")?
                .with_context(|| format!("failed writing output '{}'", point.name))?;
            }
        }
        Ok(())
    }

    fn update_metrics(
        &mut self,
        cycle_start: Instant,
        duration: Duration,
        missed: bool,
    ) -> Result<()> {
        let duration_us = duration.as_micros() as u64;
        let cycle_target_us = self.config.logic.cycle_ms * 1_000;

        let jitter_us = if let Some(prev_start) = self.last_cycle_start {
            let actual_interval = cycle_start
                .saturating_duration_since(prev_start)
                .as_micros() as i64;
            actual_interval - cycle_target_us as i64
        } else {
            0
        };

        let deadline_miss = missed || duration_us > cycle_target_us;
        if deadline_miss {
            self.deadline_misses += 1;
        }

        self.last_duration_us = duration_us;
        self.last_jitter_us = jitter_us;
        self.last_cycle_start = Some(cycle_start);

        if self.config.runtime.execution_time_test {
            self.storage.store_cycle_metric(&CycleMetric {
                timestamp_ms: now_ms(),
                duration_us,
                jitter_us,
                deadline_miss,
            })?;
        }

        Ok(())
    }

    fn append_cycle_log(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        outputs: Option<&HashMap<String, SignalValue>>,
        user_logs: &[LogicUserLog],
    ) -> Result<()> {
        let record = json!({
            "timestamp_ms": now_ms(),
            "event": "runtime_cycle",
            "controller": self.controller.snapshot(),
            "state": self.state,
            "watchdog_status": self.watchdog_status,
            "inputs": inputs,
            "outputs": outputs,
            "online_params": self.logic.current_params(),
            "user_logs": user_logs,
        });
        self.append_log_record(record)
    }

    fn store_online_param_samples(&self) -> Result<()> {
        let params = self.logic.current_params();
        let mut values = HashMap::new();
        for (key, value) in params {
            values.insert(key, SignalValue::from(value));
        }
        self.storage.store_signal_map("online_param", &values)
    }

    fn store_user_log_samples(&self, user_logs: &[LogicUserLog]) -> Result<()> {
        for entry in user_logs {
            for (signal, value) in &entry.values {
                self.storage.store_signal("user_log", signal, value)?;
            }
        }
        Ok(())
    }

    fn append_log_record(&mut self, record: serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(&record).context("failed to encode runtime log")?;
        writeln!(self.log_file, "{encoded}").context("failed writing runtime log line")?;
        self.log_file
            .flush()
            .context("failed flushing runtime log")?;
        Ok(())
    }
}

fn apply_output_limits(config: &ProjectConfig, outputs: &mut HashMap<String, SignalValue>) {
    for (name, value) in outputs.iter_mut() {
        if let Some(limit) = config.limits.values.get(name) {
            if let SignalValue::F64(v) = value {
                if *v < limit.min {
                    *v = limit.min;
                }
                if *v > limit.max {
                    *v = limit.max;
                }
            }
        }
    }
}

struct RuntimeHandle {
    controller: Arc<ControllerSignals>,
    deadline_monitor: JoinHandle<()>,
    project_name: String,
    core: Arc<Mutex<RuntimeCore>>,
    shutdown: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

struct SimulationHandle {
    project_name: String,
    core: Arc<Mutex<SimulationCore>>,
    reconfigure: Arc<Notify>,
    shutdown: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

enum ActiveHandle {
    Runtime(RuntimeHandle),
    Simulation(SimulationHandle),
}

#[derive(Default)]
pub struct RuntimeManager {
    handle: Mutex<Option<ActiveHandle>>,
    online_optimization_stop: Arc<AtomicBool>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            online_optimization_stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn start(&self, service: &ProjectService, project_name: &str) -> Result<()> {
        let mut guard = self.handle.lock().await;
        if guard.is_some() {
            bail!("runtime or simulation already running");
        }

        let config = service.load_config(project_name)?;
        config.validate()?;

        let adapter = Arc::new(OpcUaClientAdapter::new(
            config.communication.endpoint_url.clone(),
        ));
        seed_protocol_nodes(adapter.as_ref(), &config)?;

        let watchdog = build_watchdog(&config.watchdog)?;
        let project_dir = service.project_dir(project_name);
        let logic = build_logic(&config, &config.io, &project_dir)?;
        let model = build_model(config.model.as_ref());

        let data_dir = ensure_project_data_dir(&project_dir)?;
        let db_path = data_dir.join("runtime.sqlite");
        let storage = SqliteStorage::new(db_path)?;
        let log_path = data_dir.join("runtime.log");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open runtime log '{}'", log_path.display()))?;

        let mut core_value = RuntimeCore::new(
            config.clone(),
            adapter,
            watchdog,
            logic,
            model,
            storage,
            log_file,
        )
        .await?;
        core_value.online_optimization_stop = Arc::clone(&self.online_optimization_stop);
        let controller = Arc::clone(&core_value.controller);
        let deadline_monitor = tokio::spawn(Arc::clone(&controller).monitor());
        let core = Arc::new(Mutex::new(core_value));

        let (tx, mut rx) = oneshot::channel();
        let core_clone = Arc::clone(&core);
        let cycle_ms = gcd(config.communication.cycle_ms, config.logic.cycle_ms);
        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(cycle_ms));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let mut core = core_clone.lock().await;
                        if let Err(err) = core.tick_once().await {
                            tracing::error!(error = %err, "runtime tick failed");
                        }
                    }
                    _ = &mut rx => {
                        break;
                    }
                }
            }
        });

        *guard = Some(ActiveHandle::Runtime(RuntimeHandle {
            controller,
            deadline_monitor,
            project_name: project_name.to_string(),
            core,
            shutdown: tx,
            join,
        }));

        Ok(())
    }

    /// Manual enable is execution-scoped by the caller and never persisted.
    pub async fn set_controller_enabled(&self, enabled: bool) -> Result<()> {
        let guard = self.handle.lock().await;
        let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() else {
            bail!("runtime not running");
        };
        let mut core = handle.core.lock().await;
        let controller = Arc::clone(&core.controller);
        let _exchange = controller.exchange.lock().await;
        core.controller.set_enabled(enabled, false)?;
        if let Err(err) = core.controller.publish().await {
            core.controller
                .fault(format!("Controller status write error: {err:#}"));
            let _ = core.controller.publish().await;
            return Err(err);
        }
        let controller = core.controller.snapshot();
        core.append_log_record(json!({"timestamp_ms": now_ms(), "event": "controller_enable", "controller": controller}))?;
        Ok(())
    }

    pub async fn start_simulation(
        &self,
        service: &ProjectService,
        project_name: &str,
    ) -> Result<()> {
        let mut guard = self.handle.lock().await;
        if guard.is_some() {
            bail!("runtime or simulation already running");
        }

        let mut config = service.load_config(project_name)?;
        config.normalize_simulation_config();
        config.sync_active_online_params();
        config.validate_for_simulation()?;
        let (sim_io, limits) = config.build_simulation_io_and_limits()?;

        let project_dir = service.project_dir(project_name);
        let logic = build_logic(&config, &sim_io, &project_dir)?;

        let data_dir = ensure_project_data_dir(&project_dir)?;
        let db_path = data_dir.join("simulation.sqlite");
        reset_simulation_artifacts(&db_path)?;
        let storage = SqliteStorage::new(db_path)?;
        let log_path = data_dir.join("simulation.log");
        let log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .with_context(|| format!("failed to open simulation log '{}'", log_path.display()))?;

        let mut core_value =
            SimulationCore::new(config.clone(), sim_io, limits, logic, storage, log_file)?;
        core_value.online_optimization_stop = Arc::clone(&self.online_optimization_stop);
        let core = Arc::new(Mutex::new(core_value));

        let (tx, mut rx) = oneshot::channel();
        let core_clone = Arc::clone(&core);
        let reconfigure = Arc::new(Notify::new());
        let reconfigure_clone = Arc::clone(&reconfigure);
        let join = tokio::spawn(async move {
            let mut next_deadline = None;
            loop {
                let target_cycle = {
                    let mut core = core_clone.lock().await;
                    let target_cycle = core.target_cycle_duration();
                    if let Err(err) = core.tick_once(target_cycle) {
                        tracing::error!(error = %err, "simulation tick failed");
                    }
                    target_cycle
                };

                if let Some(target) = target_cycle {
                    let now = tokio::time::Instant::now();
                    let deadline = next_deadline.unwrap_or(now + target);
                    next_deadline = Some(deadline + target);

                    if deadline > now {
                        tokio::select! {
                            _ = tokio::time::sleep_until(deadline) => {}
                            _ = reconfigure_clone.notified() => {
                                next_deadline = None;
                            }
                            _ = &mut rx => break,
                        }
                    }
                } else {
                    next_deadline = None;
                    tokio::select! {
                        _ = tokio::task::yield_now() => {}
                        _ = reconfigure_clone.notified() => {}
                        _ = &mut rx => break,
                    }
                }
            }
        });

        *guard = Some(ActiveHandle::Simulation(SimulationHandle {
            project_name: project_name.to_string(),
            core,
            reconfigure,
            shutdown: tx,
            join,
        }));

        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let mut guard = self.handle.lock().await;
        let handle = match guard.take() {
            Some(ActiveHandle::Runtime(handle)) => handle,
            Some(other) => {
                *guard = Some(other);
                bail!("runtime not running");
            }
            None => return Ok(()),
        };

        // Latch shutdown before waiting on a potentially slow calculation. Its
        // eventual result cannot reassert readiness or write any actuator.
        let mut stop_result = handle.controller.stop().await;
        {
            let mut core = handle.core.lock().await;
            if let Some(mut session) = core.online_optimization.take() {
                let tick = session.stop("execution_stopped");
                stop_result = core.apply_online_optimization_tick(tick);
            }
            core.state = RuntimeState::Off;
        }
        handle.deadline_monitor.abort();
        let _ = handle.shutdown.send(());
        let _ = handle.join.await;
        stop_result
    }

    pub async fn stop_simulation(&self) -> Result<()> {
        let mut guard = self.handle.lock().await;
        let handle = match guard.take() {
            Some(ActiveHandle::Simulation(handle)) => handle,
            Some(other) => {
                *guard = Some(other);
                bail!("simulation not running");
            }
            None => return Ok(()),
        };

        {
            let mut core = handle.core.lock().await;
            if let Some(mut session) = core.online_optimization.take() {
                let tick = session.stop("execution_stopped");
                core.apply_online_optimization_tick(tick)?;
            }
        }
        let _ = handle.shutdown.send(());
        let _ = handle.join.await;
        Ok(())
    }

    pub async fn state(&self) -> Result<Option<RuntimeSnapshot>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() {
            let core = handle.core.lock().await;
            Ok(Some(core.snapshot()))
        } else {
            Ok(None)
        }
    }

    pub async fn simulation_state(&self) -> Result<Option<SimulationSnapshot>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Simulation(handle)) = guard.as_ref() {
            let core = handle.core.lock().await;
            Ok(Some(core.snapshot()))
        } else {
            Ok(None)
        }
    }

    pub async fn active_state(&self) -> Result<Option<ExecutionSnapshot>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                let core = handle.core.lock().await;
                Ok(Some(ExecutionSnapshot::Runtime(core.snapshot())))
            }
            Some(ActiveHandle::Simulation(handle)) => {
                let core = handle.core.lock().await;
                Ok(Some(ExecutionSnapshot::Simulation(core.snapshot())))
            }
            None => Ok(None),
        }
    }

    pub async fn active_project(&self) -> Option<String> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => Some(handle.project_name.clone()),
            Some(ActiveHandle::Simulation(handle)) => Some(handle.project_name.clone()),
            None => None,
        }
    }

    pub async fn set_params(
        &self,
        updates: HashMap<String, LogicParamValue>,
    ) -> Result<HashMap<String, LogicParamValue>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() {
            let mut core = handle.core.lock().await;
            core.set_params(updates)?;
            Ok(core.current_params())
        } else {
            bail!("runtime not running")
        }
    }

    pub async fn active_set_params(
        &self,
        updates: HashMap<String, LogicParamValue>,
    ) -> Result<HashMap<String, LogicParamValue>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                let mut core = handle.core.lock().await;
                core.set_params(updates)?;
                Ok(core.current_params())
            }
            Some(ActiveHandle::Simulation(handle)) => {
                let mut core = handle.core.lock().await;
                core.set_params(updates)?;
                Ok(core.current_params())
            }
            None => bail!("runtime or simulation not running"),
        }
    }

    pub async fn get_params(&self) -> Result<HashMap<String, LogicParamValue>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() {
            let core = handle.core.lock().await;
            Ok(core.current_params())
        } else {
            bail!("runtime not running")
        }
    }

    pub async fn active_get_params(&self) -> Result<HashMap<String, LogicParamValue>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                let core = handle.core.lock().await;
                Ok(core.current_params())
            }
            Some(ActiveHandle::Simulation(handle)) => {
                let core = handle.core.lock().await;
                Ok(core.current_params())
            }
            None => bail!("runtime or simulation not running"),
        }
    }

    pub async fn start_online_optimization(
        &self,
        service: &ProjectService,
        project_name: &str,
    ) -> Result<OnlineOptimizationStatus> {
        let mut config = service.load_config(project_name)?;
        config.sync_active_online_params();
        config.normalize_online_optimization_config();
        config.validate()?;
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) if handle.project_name == project_name => {
                let mut core = handle.core.lock().await;
                if core.online_optimization_status().is_some_and(|status| {
                    status.phase == crate::online_optimization::OnlineOptimizationPhase::Finished
                }) {
                    core.online_optimization = None;
                }
                core.start_online_optimization(config.online_optimization)?;
                Ok(core
                    .online_optimization_status()
                    .expect("session just started"))
            }
            Some(ActiveHandle::Simulation(handle)) if handle.project_name == project_name => {
                let mut core = handle.core.lock().await;
                if core.online_optimization_status().is_some_and(|status| {
                    status.phase == crate::online_optimization::OnlineOptimizationPhase::Finished
                }) {
                    core.online_optimization = None;
                }
                core.start_online_optimization(config.online_optimization)?;
                Ok(core
                    .online_optimization_status()
                    .expect("session just started"))
            }
            Some(_) => bail!("online optimization project does not match active execution"),
            None => bail!("runtime or simulation must be running"),
        }
    }

    pub async fn online_optimization_status(&self) -> Result<Option<OnlineOptimizationStatus>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                Ok(handle.core.lock().await.online_optimization_status())
            }
            Some(ActiveHandle::Simulation(handle)) => {
                Ok(handle.core.lock().await.online_optimization_status())
            }
            None => Ok(None),
        }
    }

    pub async fn stop_online_optimization(&self) -> Result<()> {
        self.online_optimization_stop.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn read_plots(&self, limit: usize) -> Result<Vec<SignalSample>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() {
            let core = handle.core.lock().await;
            core.read_plot_samples(limit)
        } else {
            bail!("runtime not running")
        }
    }

    pub async fn read_plots_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() {
            let core = handle.core.lock().await;
            core.read_plot_samples_window(window_ms)
        } else {
            bail!("runtime not running")
        }
    }

    pub async fn read_plot_catalog(&self) -> Result<Vec<SeriesCatalogEntry>> {
        let guard = self.handle.lock().await;
        if let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() {
            let core = handle.core.lock().await;
            core.read_plot_catalog()
        } else {
            bail!("runtime not running")
        }
    }

    pub async fn active_read_plots(&self, limit: usize) -> Result<Vec<SignalSample>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                let core = handle.core.lock().await;
                core.read_plot_samples(limit)
            }
            Some(ActiveHandle::Simulation(handle)) => {
                let core = handle.core.lock().await;
                core.read_plot_samples(limit)
            }
            None => bail!("runtime or simulation not running"),
        }
    }

    pub async fn active_read_plots_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                let core = handle.core.lock().await;
                core.read_plot_samples_window(window_ms)
            }
            Some(ActiveHandle::Simulation(handle)) => {
                let core = handle.core.lock().await;
                core.read_plot_samples_window(window_ms)
            }
            None => bail!("runtime or simulation not running"),
        }
    }

    pub async fn active_read_plot_catalog(&self) -> Result<Vec<SeriesCatalogEntry>> {
        let guard = self.handle.lock().await;
        match guard.as_ref() {
            Some(ActiveHandle::Runtime(handle)) => {
                let core = handle.core.lock().await;
                core.read_plot_catalog()
            }
            Some(ActiveHandle::Simulation(handle)) => {
                let core = handle.core.lock().await;
                core.read_plot_catalog()
            }
            None => bail!("runtime or simulation not running"),
        }
    }

    pub async fn apply_simulation_live_config(&self, config: SimulationConfig) -> Result<()> {
        let guard = self.handle.lock().await;
        let Some(ActiveHandle::Simulation(handle)) = guard.as_ref() else {
            bail!("simulation not running");
        };
        let mut core = handle.core.lock().await;
        if core.online_optimization_status().is_some_and(|status| {
            status.phase != crate::online_optimization::OnlineOptimizationPhase::Finished
        }) {
            bail!("simulation live configuration is locked during online optimization");
        }
        core.apply_live_config(&config);
        handle.reconfigure.notify_one();
        Ok(())
    }

    pub async fn simulate_step(
        &self,
        inputs: HashMap<String, SignalValue>,
    ) -> Result<HashMap<String, SignalValue>> {
        let guard = self.handle.lock().await;
        let Some(ActiveHandle::Runtime(handle)) = guard.as_ref() else {
            bail!("runtime not running");
        };

        let mut core = handle.core.lock().await;
        core.simulate_step(inputs).await
    }
}

fn build_watchdog(cfg: &WatchdogConfig) -> Result<Box<dyn WatchdogModule>> {
    match cfg {
        WatchdogConfig::None => Ok(Box::new(NoneWatchdog)),
        WatchdogConfig::CounterMirror {
            read_node,
            write_node,
            stale_timeout_ms,
        } => Ok(Box::new(CounterMirrorWatchdog::new(
            read_node.clone(),
            write_node.clone(),
            stale_timeout_or_default(*stale_timeout_ms)?,
        ))),
    }
}

fn cycle_due(last: Option<Instant>, now: Instant, cycle_ms: u64) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_millis(cycle_ms))
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

pub(crate) fn build_logic(
    config: &ProjectConfig,
    io: &IoConfig,
    project_dir: &Path,
) -> Result<Box<dyn ControlLogic>> {
    match config.logic.kind {
        LogicKind::PController => {
            let logic = PControllerLogic::new(
                config.logic.p_controller.pairs.clone(),
                config.logic.params.clone(),
            );
            if !logic.is_configured() {
                bail!("P controller has no configured CV-to-MV pairs");
            }
            Ok(Box::new(logic))
        }
        LogicKind::LinearMpc => {
            if !config.linear_mpc_is_configured_for(io) {
                bail!(
                    "Linear MPC strategy is not compatible with the current model and I/O definition"
                );
            }
            let model = config
                .model
                .as_ref()
                .and_then(ModelConfig::linear_state_space)
                .cloned()
                .ok_or_else(|| anyhow!("Linear MPC requires a linear state-space model"))?;
            let logic = LinearMpcLogic::new(
                model,
                config.variables.clone(),
                config.logic.cycle_ms,
                config.logic.params.clone(),
                config.static_cv_band_defaults(),
            );
            if !logic.is_configured() {
                bail!("Linear MPC has no usable MV/CV model configuration");
            }
            Ok(Box::new(logic))
        }
        LogicKind::CustomRust => {
            if !config.logic.custom_rust.is_compatible_with(io) {
                bail!("Custom Rust strategy is not compatible with the current I/O definition");
            }
            let artifact_path = if Path::new(&config.logic.custom_rust.artifact_path).is_absolute()
            {
                PathBuf::from(&config.logic.custom_rust.artifact_path)
            } else {
                project_dir.join(&config.logic.custom_rust.artifact_path)
            };
            let logic = CustomRustLogic::load(
                artifact_path,
                config.logic.params.clone(),
                config.logic.cycle_ms as f64 / 1_000.0,
            )?;
            Ok(Box::new(logic))
        }
    }
}

fn build_model(model_config: Option<&ModelConfig>) -> Option<Box<dyn ModelModule>> {
    match model_config {
        Some(ModelConfig::Noop { .. }) | Some(ModelConfig::LinearStateSpace { .. }) | None => None,
    }
}

pub(crate) fn seed_protocol_nodes(
    adapter: &OpcUaClientAdapter,
    config: &ProjectConfig,
) -> Result<()> {
    for (_, node) in config.deployment.signal_nodes() {
        adapter.seed_node(node, SignalValue::Bool(false));
    }
    for input in &config.io.inputs {
        let value = match input.signal_type {
            SignalType::F64 => SignalValue::F64(0.0),
            SignalType::Bool => SignalValue::Bool(false),
        };
        adapter.seed_node(input.address.clone(), value);
    }

    for output in &config.io.outputs {
        let value = match output.signal_type {
            SignalType::F64 => SignalValue::F64(0.0),
            SignalType::Bool => SignalValue::Bool(false),
        };
        adapter.seed_node(output.address.clone(), value);
    }

    if let WatchdogConfig::CounterMirror {
        read_node,
        write_node,
        ..
    } = &config.watchdog
    {
        adapter.seed_node(read_node.clone(), SignalValue::F64(0.0));
        adapter.seed_node(write_node.clone(), SignalValue::F64(0.0));
    }

    Ok(())
}

fn ensure_project_data_dir(project_dir: &Path) -> Result<PathBuf> {
    let data_dir = project_dir.join("data");
    fs::create_dir_all(&data_dir).with_context(|| {
        format!(
            "failed to create project data directory '{}'",
            data_dir.display()
        )
    })?;
    Ok(data_dir)
}

fn reset_simulation_artifacts(db_path: &Path) -> Result<()> {
    if db_path.exists() {
        fs::remove_file(db_path).with_context(|| {
            format!(
                "failed to remove previous simulation database '{}'",
                db_path.display()
            )
        })?;
    }

    for suffix in ["-shm", "-wal"] {
        let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if sidecar.exists() {
            fs::remove_file(&sidecar).with_context(|| {
                format!(
                    "failed to remove previous simulation database sidecar '{}'",
                    sidecar.display()
                )
            })?;
        }
    }

    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, time::Duration};

    use tempfile::tempdir;

    use crate::{
        communication::opcua::OpcUaClientAdapter,
        logic::{ControlLogic, LogicMetadata, ParameterSchemaEntry},
        project_config::{
            AlarmConfig, LogicConfig, LogicKind, LogicParamValue, PControllerLogicConfig,
            PControllerPairConfig, SignalPoint, WatchdogConfig,
        },
    };

    use super::*;

    fn base_config() -> ProjectConfig {
        let mut cfg = ProjectConfig::new("test_project");
        cfg.communication.endpoint_url = "mock://runtime-tests".to_string();
        cfg.io.inputs = vec![
            SignalPoint {
                name: "cv.y1.setpoint".to_string(),
                signal_type: SignalType::F64,
                address: "ns=2;s=Y1_SETPOINT".to_string(),
                opc_tag: None,
            },
            SignalPoint {
                name: "cv.y1.process_value".to_string(),
                signal_type: SignalType::F64,
                address: "ns=2;s=Y1_PROCESS_VALUE".to_string(),
                opc_tag: None,
            },
        ];
        cfg.io.outputs = vec![SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=U1_WRITE".to_string(),
            opc_tag: None,
        }];
        cfg.logic = LogicConfig {
            cycle_ms: 1_000,
            kind: LogicKind::PController,
            p_controller: PControllerLogicConfig {
                pairs: vec![PControllerPairConfig {
                    cv: "y1".to_string(),
                    mv: "u1".to_string(),
                    kp: 2.0,
                    bias: 0.0,
                }],
            },
            custom_rust: Default::default(),
            params: HashMap::new(),
            initial_values: HashMap::new(),
        };
        cfg.alarms = AlarmConfig {
            alarm_timeout_ms: 100,
            off_timeout_ms: 200,
        };
        cfg.watchdog = WatchdogConfig::None;
        cfg.logic.cycle_ms = 1_000;
        cfg
    }

    #[derive(Default)]
    struct LoggingTestLogic {
        delay: Duration,
        fail: bool,
        converged_after: usize,
        calls: usize,
        params: HashMap<String, LogicParamValue>,
        pending_logs: Vec<LogicUserLog>,
    }

    impl ControlLogic for LoggingTestLogic {
        fn observer_converged(&self) -> bool {
            self.calls >= self.converged_after
        }
        fn metadata(&self) -> LogicMetadata {
            LogicMetadata {
                kind: "test_logging".to_string(),
                description: "test logic".to_string(),
            }
        }

        fn init(
            &mut self,
            _initial_values: HashMap<String, SignalValue>,
            params: HashMap<String, LogicParamValue>,
        ) {
            self.params = params;
        }

        fn step(
            &mut self,
            _inputs: &HashMap<String, SignalValue>,
            _state: RuntimeState,
        ) -> Result<HashMap<String, SignalValue>> {
            self.calls += 1;
            std::thread::sleep(self.delay);
            if self.fail {
                bail!("injected controller error");
            }
            self.pending_logs = vec![LogicUserLog {
                message: "runtime log values".to_string(),
                values: HashMap::from([("ff.signal".to_string(), SignalValue::F64(7.0))]),
            }];
            Ok(HashMap::from([(
                "mv.u1.write".to_string(),
                SignalValue::F64(3.0),
            )]))
        }

        fn parameter_schema(&self) -> Vec<ParameterSchemaEntry> {
            Vec::new()
        }

        fn set_params(&mut self, params: HashMap<String, LogicParamValue>) {
            self.params = params;
        }

        fn current_params(&self) -> HashMap<String, LogicParamValue> {
            self.params.clone()
        }

        fn drain_user_logs(&mut self) -> Vec<LogicUserLog> {
            std::mem::take(&mut self.pending_logs)
        }
    }

    async fn handshake_core(
        mut cfg: ProjectConfig,
        logic: Box<dyn ControlLogic>,
    ) -> (RuntimeCore, Arc<OpcUaClientAdapter>, tempfile::TempDir) {
        cfg.deployment.controller_is_ready = Some("ns=2;s=ControllerIsReady".into());
        cfg.deployment.controller_has_control = Some("ns=2;s=ControllerHasControl".into());
        let adapter = Arc::new(OpcUaClientAdapter::new("mock://handshake"));
        seed_protocol_nodes(&adapter, &cfg).unwrap();
        let dir = tempdir().unwrap();
        let storage = SqliteStorage::new(dir.path().join("runtime.sqlite")).unwrap();
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.path().join("runtime.log"))
            .unwrap();
        let core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            logic,
            None,
            storage,
            log_file,
        )
        .await
        .unwrap();
        (core, adapter, dir)
    }

    async fn assert_flags(adapter: &OpcUaClientAdapter, ready: bool, control: bool) {
        assert_eq!(
            adapter.read("ns=2;s=ControllerIsReady").await.unwrap(),
            SignalValue::Bool(ready)
        );
        assert_eq!(
            adapter.read("ns=2;s=ControllerHasControl").await.unwrap(),
            SignalValue::Bool(control)
        );
    }

    #[tokio::test]
    async fn handshake_starts_shadow_waits_for_observer_and_requires_enable() {
        let (mut core, adapter, _dir) = handshake_core(
            base_config(),
            Box::new(LoggingTestLogic {
                converged_after: 2,
                ..Default::default()
            }),
        )
        .await;
        assert_flags(&adapter, false, false).await;
        core.tick_once().await.unwrap();
        assert_flags(&adapter, false, false).await;
        core.last_controller_cycle = None;
        core.last_io_cycle = None;
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, false).await;
        assert_eq!(
            adapter.read("ns=2;s=U1_WRITE").await.unwrap(),
            SignalValue::F64(0.0)
        );
        core.controller.set_enabled(true, false).unwrap();
        core.last_io_cycle = None;
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, true).await;
        assert_eq!(
            adapter.read("ns=2;s=U1_WRITE").await.unwrap(),
            SignalValue::F64(3.0)
        );
        core.controller.set_enabled(false, false).unwrap();
        core.last_io_cycle = None;
        adapter.seed_node("ns=2;s=U1_WRITE", SignalValue::F64(9.0));
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, false).await;
        assert_eq!(
            adapter.read("ns=2;s=U1_WRITE").await.unwrap(),
            SignalValue::F64(9.0)
        );
    }

    #[tokio::test]
    async fn external_enable_is_boolean_read_only_and_locks_manual_enable() {
        let mut cfg = base_config();
        cfg.deployment.external_controller_enable = Some("ns=2;s=ExternalControllerEnable".into());
        let (mut core, adapter, _dir) =
            handshake_core(cfg, Box::new(LoggingTestLogic::default())).await;
        assert!(core.controller.set_enabled(true, false).is_err());
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, false).await;
        adapter.seed_node("ns=2;s=ExternalControllerEnable", SignalValue::Bool(true));
        core.last_io_cycle = None;
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, true).await;
        assert_eq!(
            adapter
                .read("ns=2;s=ExternalControllerEnable")
                .await
                .unwrap(),
            SignalValue::Bool(true)
        );
        adapter.seed_node("ns=2;s=ExternalControllerEnable", SignalValue::F64(1.0));
        core.last_io_cycle = None;
        assert!(core.tick_once().await.is_err());
        assert_flags(&adapter, false, false).await;
        assert!(!core.controller.snapshot().enabled);
    }

    #[tokio::test]
    async fn controller_error_clears_ready_and_control_and_does_not_rewrite_old_output() {
        let (mut core, adapter, _dir) =
            handshake_core(base_config(), Box::new(LoggingTestLogic::default())).await;
        core.controller.set_enabled(true, false).unwrap();
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, true).await;
        core.logic = Box::new(LoggingTestLogic {
            fail: true,
            ..Default::default()
        });
        core.last_controller_cycle = None;
        assert!(core.tick_once().await.is_err());
        assert_flags(&adapter, false, false).await;
        adapter.seed_node("ns=2;s=U1_WRITE", SignalValue::F64(9.0));
        core.last_io_cycle = None;
        core.last_controller_cycle = Some(Instant::now());
        core.tick_once().await.unwrap();
        assert_eq!(
            adapter.read("ns=2;s=U1_WRITE").await.unwrap(),
            SignalValue::F64(9.0)
        );
    }

    #[tokio::test]
    async fn deadline_monitor_revokes_flags_while_slow_controller_is_still_running() {
        let mut cfg = base_config();
        cfg.logic.cycle_ms = 50;
        let (mut core, adapter, _dir) =
            handshake_core(cfg, Box::new(LoggingTestLogic::default())).await;
        core.controller.set_enabled(true, false).unwrap();
        core.tick_once().await.unwrap();
        assert_flags(&adapter, true, true).await;
        let controller = Arc::clone(&core.controller);
        let monitor = tokio::spawn(Arc::clone(&controller).monitor());
        core.logic = Box::new(LoggingTestLogic {
            delay: Duration::from_millis(200),
            ..Default::default()
        });
        core.last_controller_cycle = Some(Instant::now() - Duration::from_millis(50));
        let worker = tokio::spawn(async move {
            core.tick_once().await.unwrap();
            core
        });
        tokio::time::sleep(Duration::from_millis(90)).await;
        assert!(!worker.is_finished());
        assert_flags(&adapter, false, false).await;
        let core = worker.await.unwrap();
        assert!(!core.controller.snapshot().is_ready);
        assert!(core.deadline_misses > 0);
        monitor.abort();
    }

    #[tokio::test]
    async fn limits_are_applied() {
        let mut cfg = base_config();
        cfg.limits.values.insert(
            "mv.u1.write".to_string(),
            crate::project_config::NumericLimit {
                min: -1.0,
                max: 1.0,
                opc_tag: None,
            },
        );

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-limits"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed nodes");
        adapter.seed_node("ns=2;s=Y1_SETPOINT", SignalValue::F64(10.0));
        adapter.seed_node("ns=2;s=Y1_PROCESS_VALUE", SignalValue::F64(0.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(storage_dir.path().join("runtime.log"))
            .expect("log");
        let logic_cfg = base_config();
        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            build_logic(&logic_cfg, &logic_cfg.io, storage_dir.path()).expect("build logic"),
            None,
            storage,
            log_file,
        )
        .await
        .expect("runtime core");

        core.controller
            .set_enabled(true, false)
            .expect("explicit enable");
        core.tick_once().await.expect("tick");

        let written = adapter.read("ns=2;s=U1_WRITE").await.expect("read output");
        assert_eq!(written, SignalValue::F64(1.0));
    }

    #[tokio::test]
    async fn fault_state_machine_transitions_hold_alarm_off() {
        let mut cfg = base_config();
        cfg.communication.cycle_ms = 1;
        cfg.alarms.alarm_timeout_ms = 40;
        cfg.alarms.off_timeout_ms = 80;

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-fault"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=Y1_SETPOINT", SignalValue::F64(1.0));
        adapter.seed_node("ns=2;s=Y1_PROCESS_VALUE", SignalValue::F64(0.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(storage_dir.path().join("runtime.log"))
            .expect("log");
        let logic_cfg = base_config();
        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            build_logic(&logic_cfg, &logic_cfg.io, storage_dir.path()).expect("build logic"),
            None,
            storage,
            log_file,
        )
        .await
        .expect("runtime core");

        adapter.set_force_fault(true);

        core.tick_once().await.expect("hold");
        assert_eq!(core.state, RuntimeState::Hold);

        tokio::time::sleep(Duration::from_millis(45)).await;
        core.tick_once().await.expect("alarm");
        assert_eq!(core.state, RuntimeState::Alarm);

        tokio::time::sleep(Duration::from_millis(45)).await;
        core.tick_once().await.expect("off");
        assert_eq!(core.state, RuntimeState::Off);
    }

    #[tokio::test]
    async fn runtime_writes_cycle_and_param_logs_to_file() {
        let cfg = base_config();

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-logging"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=Y1_SETPOINT", SignalValue::F64(2.0));
        adapter.seed_node("ns=2;s=Y1_PROCESS_VALUE", SignalValue::F64(1.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let log_path = storage_dir.path().join("runtime.log");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("log");
        let logic_cfg = base_config();

        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            build_logic(&logic_cfg, &logic_cfg.io, storage_dir.path()).expect("logic"),
            None,
            storage,
            log_file,
        )
        .await
        .expect("runtime");

        core.tick_once().await.expect("tick");
        let mut updates = HashMap::new();
        updates.insert("p.y1.u1.kp".to_string(), LogicParamValue::F64(3.0));
        core.set_params(updates).expect("set params");

        let content = fs::read_to_string(&log_path).expect("read log");
        assert!(content.contains("\"event\":\"runtime_cycle\""));
        assert!(content.contains("\"event\":\"online_params_update\""));
        assert!(content.contains("cv.y1.setpoint"));
        assert!(content.contains("mv.u1.write"));
    }

    #[tokio::test]
    async fn controller_output_waits_for_next_io_cycle() {
        let mut cfg = base_config();
        cfg.communication.cycle_ms = 100;
        cfg.io.cycle_ms = 200;
        cfg.logic.cycle_ms = 50;

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-independent-cycles"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=Y1_SETPOINT", SignalValue::F64(2.0));
        adapter.seed_node("ns=2;s=Y1_PROCESS_VALUE", SignalValue::F64(1.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(storage_dir.path().join("runtime.log"))
            .expect("log");
        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            Box::new(LoggingTestLogic::default()),
            None,
            storage,
            log_file,
        )
        .await
        .expect("runtime");

        core.controller
            .set_enabled(true, false)
            .expect("explicit enable");
        core.tick_once().await.expect("initial coincident tick");
        adapter
            .write("ns=2;s=U1_WRITE", SignalValue::F64(9.0))
            .await
            .expect("external output seed");

        let now = Instant::now();
        core.last_communication_cycle = Some(now);
        core.last_io_cycle = Some(now);
        core.last_controller_cycle = Some(now - Duration::from_millis(50));
        core.tick_once().await.expect("controller-only tick");
        assert_eq!(
            adapter.read("ns=2;s=U1_WRITE").await.expect("read output"),
            SignalValue::F64(9.0)
        );

        let now = Instant::now();
        core.last_communication_cycle = Some(now);
        core.last_io_cycle = Some(now - Duration::from_millis(200));
        core.last_controller_cycle = Some(now);
        core.tick_once().await.expect("io-only tick");
        assert_eq!(
            adapter.read("ns=2;s=U1_WRITE").await.expect("read output"),
            SignalValue::F64(3.0)
        );
    }

    #[tokio::test]
    async fn runtime_stores_online_params_and_user_log_values_as_plot_samples() {
        let mut cfg = base_config();
        cfg.logic
            .params
            .insert("custom_gain".to_string(), LogicParamValue::F64(1.5));
        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-plot-extra"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=Y1_SETPOINT", SignalValue::F64(2.0));
        adapter.seed_node("ns=2;s=Y1_PROCESS_VALUE", SignalValue::F64(1.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(storage_dir.path().join("runtime.log"))
            .expect("log");

        let mut core = RuntimeCore::new(
            cfg,
            adapter,
            Box::new(NoneWatchdog),
            Box::new(LoggingTestLogic::default()),
            None,
            storage,
            log_file,
        )
        .await
        .expect("runtime core");

        core.tick_once().await.expect("tick");

        let rows = core.read_plot_samples(200).expect("samples");
        assert!(
            rows.iter()
                .any(|s| s.direction == "online_param" && s.signal == "custom_gain")
        );
        assert!(
            rows.iter()
                .any(|s| s.direction == "user_log" && s.signal == "ff.signal")
        );
    }
}
