use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::json;
use tokio::{
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

use crate::{
    communication::{HealthStatus, ProtocolAdapter, opcua::OpcUaClientAdapter},
    logic::{ControlLogic, CustomRustLogic, LogicUserLog, PControllerLogic},
    model::ModelModule,
    project::ProjectService,
    project_config::{LogicKind, LogicParamValue, ModelConfig, ProjectConfig, WatchdogConfig},
    storage::{CycleMetric, SeriesCatalogEntry, SignalSample, SqliteStorage},
    types::{RuntimeState, SignalType, SignalValue},
    watchdog::{
        CounterMirrorWatchdog, NoneWatchdog, WatchdogModule, WatchdogStatus,
        mirror_cycle_or_default, stale_timeout_or_default,
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
    pub project_name: String,
    pub state: RuntimeState,
    pub watchdog_status: WatchdogStatus,
    pub metrics: RuntimeMetrics,
    pub last_inputs: HashMap<String, SignalValue>,
    pub last_outputs: HashMap<String, SignalValue>,
}

struct RuntimeCore {
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
    last_inputs: HashMap<String, SignalValue>,
    last_outputs: HashMap<String, SignalValue>,
    total_cycles: u64,
    deadline_misses: u64,
    last_duration_us: u64,
    last_jitter_us: i64,
    last_cycle_start: Option<Instant>,
    last_io_exchange: Option<Instant>,
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
        watchdog.init(protocol.as_ref()).await?;
        logic.init(
            config.logic.initial_values.clone(),
            config.logic.params.clone(),
        );

        Ok(Self {
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
            last_inputs: HashMap::new(),
            last_outputs: HashMap::new(),
            total_cycles: 0,
            deadline_misses: 0,
            last_duration_us: 0,
            last_jitter_us: 0,
            last_cycle_start: None,
            last_io_exchange: None,
        })
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            project_name: self.config.name.clone(),
            state: self.state,
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

    fn read_plot_samples(&self, limit: usize) -> Result<Vec<SignalSample>> {
        self.storage.read_recent_samples(limit)
    }

    fn read_plot_samples_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        let since = now_ms().saturating_sub(window_ms as i64);
        self.storage.read_samples_since(since)
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
        let cycle_start = Instant::now();
        let should_exchange_io = self.should_exchange_io(cycle_start);

        let (inputs, input_ok) = if should_exchange_io {
            match self.read_inputs().await {
                Ok(inputs) => {
                    self.storage.store_signal_map("input", &inputs)?;
                    self.last_inputs = inputs.clone();
                    (inputs, true)
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed reading inputs; entering fault path");
                    (self.last_inputs.clone(), false)
                }
            }
        } else {
            (self.last_inputs.clone(), true)
        };

        let watchdog_status = match self.watchdog.tick(self.protocol.as_ref()).await {
            Ok(status) => status,
            Err(err) => WatchdogStatus::Fault {
                reason: format!("watchdog tick failed: {err}"),
            },
        };
        self.watchdog_status = watchdog_status.clone();

        let healthy_protocol = self.protocol.health() == HealthStatus::Healthy && input_ok;
        let healthy_watchdog = !matches!(&watchdog_status, WatchdogStatus::Fault { .. });
        self.state = self.compute_next_state(healthy_protocol && healthy_watchdog);

        let mut cycle_outputs: Option<HashMap<String, SignalValue>> = None;
        let mut user_logs: Vec<LogicUserLog> = Vec::new();

        match self.state {
            RuntimeState::Running => {
                let mut outputs = self.logic.step(&inputs, self.state)?;
                user_logs = self.logic.drain_user_logs();
                apply_output_limits(&self.config, &mut outputs);
                self.last_outputs = outputs.clone();
                cycle_outputs = Some(outputs.clone());
                if should_exchange_io {
                    self.write_outputs(&outputs).await?;
                    self.storage.store_signal_map("output", &outputs)?;
                }
            }
            RuntimeState::Hold | RuntimeState::Alarm => {
                if should_exchange_io && !self.last_outputs.is_empty() {
                    self.write_outputs(&self.last_outputs).await?;
                    self.storage
                        .store_signal_map("output", &self.last_outputs)?;
                    cycle_outputs = Some(self.last_outputs.clone());
                }
            }
            RuntimeState::Off => {
                // Off-state explicitly avoids writing outputs.
            }
        }

        if should_exchange_io {
            self.store_online_param_samples()?;
            self.store_user_log_samples(&user_logs)?;
            self.last_io_exchange = Some(cycle_start);
            self.append_cycle_log(&inputs, cycle_outputs.as_ref(), &user_logs)?;
        }

        let duration = cycle_start.elapsed();
        self.update_metrics(cycle_start, duration)?;
        self.total_cycles += 1;

        Ok(())
    }

    fn should_exchange_io(&self, cycle_start: Instant) -> bool {
        match self.last_io_exchange {
            None => true,
            Some(last) => {
                cycle_start.saturating_duration_since(last)
                    >= Duration::from_millis(self.config.communication.io_cycle_ms)
            }
        }
    }

    async fn read_inputs(&self) -> Result<HashMap<String, SignalValue>> {
        let mut inputs = HashMap::new();
        for point in &self.config.io.inputs {
            let value = self
                .protocol
                .read(&point.address)
                .await
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
            if let Some(value) = outputs.get(&point.name) {
                self.protocol
                    .write(&point.address, value.clone())
                    .await
                    .with_context(|| format!("failed writing output '{}'", point.name))?;
            }
        }
        Ok(())
    }

    fn update_metrics(&mut self, cycle_start: Instant, duration: Duration) -> Result<()> {
        let duration_us = duration.as_micros() as u64;
        let cycle_target_us = self.config.runtime.cycle_ms * 1_000;

        let jitter_us = if let Some(prev_start) = self.last_cycle_start {
            let actual_interval = cycle_start
                .saturating_duration_since(prev_start)
                .as_micros() as i64;
            actual_interval - cycle_target_us as i64
        } else {
            0
        };

        let deadline_miss = duration_us > cycle_target_us;
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
    project_name: String,
    core: Arc<Mutex<RuntimeCore>>,
    shutdown: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

#[derive(Default)]
pub struct RuntimeManager {
    handle: Mutex<Option<RuntimeHandle>>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }

    pub async fn start(&self, service: &ProjectService, project_name: &str) -> Result<()> {
        let mut guard = self.handle.lock().await;
        if guard.is_some() {
            bail!("runtime already running");
        }

        let config = service.load_config(project_name)?;
        config.validate()?;

        let adapter = Arc::new(OpcUaClientAdapter::new(
            config.communication.endpoint_url.clone(),
        ));
        seed_protocol_nodes(adapter.as_ref(), &config)?;

        let watchdog = build_watchdog(&config.watchdog)?;
        let project_dir = service.project_dir(project_name);
        let logic = build_logic(&config, &project_dir)?;
        let model = build_model(config.model.as_ref());

        let db_path = project_dir.join("data").join("runtime.sqlite");
        let storage = SqliteStorage::new(db_path)?;
        let log_path = project_dir.join("data").join("runtime.log");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open runtime log '{}'", log_path.display()))?;

        let core = Arc::new(Mutex::new(
            RuntimeCore::new(
                config.clone(),
                adapter,
                watchdog,
                logic,
                model,
                storage,
                log_file,
            )
            .await?,
        ));

        let (tx, mut rx) = oneshot::channel();
        let core_clone = Arc::clone(&core);
        let cycle_ms = config.runtime.cycle_ms;
        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(cycle_ms));

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

        *guard = Some(RuntimeHandle {
            project_name: project_name.to_string(),
            core,
            shutdown: tx,
            join,
        });

        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let mut guard = self.handle.lock().await;
        let handle = guard.take();

        if let Some(handle) = handle {
            let _ = handle.shutdown.send(());
            let _ = handle.join.await;
        }
        Ok(())
    }

    pub async fn state(&self) -> Result<Option<RuntimeSnapshot>> {
        let guard = self.handle.lock().await;
        if let Some(handle) = guard.as_ref() {
            let core = handle.core.lock().await;
            Ok(Some(core.snapshot()))
        } else {
            Ok(None)
        }
    }

    pub async fn active_project(&self) -> Option<String> {
        let guard = self.handle.lock().await;
        guard.as_ref().map(|h| h.project_name.clone())
    }

    pub async fn set_params(
        &self,
        updates: HashMap<String, LogicParamValue>,
    ) -> Result<HashMap<String, LogicParamValue>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

        let mut core = handle.core.lock().await;
        core.set_params(updates)?;
        Ok(core.current_params())
    }

    pub async fn get_params(&self) -> Result<HashMap<String, LogicParamValue>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

        let core = handle.core.lock().await;
        Ok(core.current_params())
    }

    pub async fn read_plots(&self, limit: usize) -> Result<Vec<SignalSample>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

        let core = handle.core.lock().await;
        core.read_plot_samples(limit)
    }

    pub async fn read_plots_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

        let core = handle.core.lock().await;
        core.read_plot_samples_window(window_ms)
    }

    pub async fn read_plot_catalog(&self) -> Result<Vec<SeriesCatalogEntry>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

        let core = handle.core.lock().await;
        core.read_plot_catalog()
    }

    pub async fn simulate_step(
        &self,
        inputs: HashMap<String, SignalValue>,
    ) -> Result<HashMap<String, SignalValue>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

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
            mirror_cycle_ms,
            stale_timeout_ms,
        } => Ok(Box::new(CounterMirrorWatchdog::new(
            read_node.clone(),
            write_node.clone(),
            mirror_cycle_or_default(*mirror_cycle_ms)?,
            stale_timeout_or_default(*stale_timeout_ms)?,
        ))),
    }
}

pub(crate) fn build_logic(
    config: &ProjectConfig,
    project_dir: &Path,
) -> Result<Box<dyn ControlLogic>> {
    match config.logic.kind {
        LogicKind::PController => {
            let logic = PControllerLogic::new(
                config.logic.p_controller.pairs.clone(),
                config.logic.params.clone(),
            );
            if !logic.is_configured() {
                bail!("p_controller has no CV->MV pairs configured");
            }
            Ok(Box::new(logic))
        }
        LogicKind::CustomRust => {
            if !config.logic.custom_rust.is_compatible_with(&config.io) {
                bail!("custom_rust strategy is not compatible with current I/O definition");
            }
            let artifact_path = if Path::new(&config.logic.custom_rust.artifact_path).is_absolute()
            {
                PathBuf::from(&config.logic.custom_rust.artifact_path)
            } else {
                project_dir.join(&config.logic.custom_rust.artifact_path)
            };
            let logic = CustomRustLogic::load(artifact_path, config.logic.params.clone())?;
            Ok(Box::new(logic))
        }
    }
}

fn build_model(model_config: Option<&ModelConfig>) -> Option<Box<dyn ModelModule>> {
    match model_config {
        Some(ModelConfig::Noop { .. }) | Some(ModelConfig::LinearStateSpace { .. }) | None => None,
    }
}

fn seed_protocol_nodes(adapter: &OpcUaClientAdapter, config: &ProjectConfig) -> Result<()> {
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
                name: "cv.y1.soll".to_string(),
                signal_type: SignalType::F64,
                address: "ns=2;s=Y1_SOLL".to_string(),
                opc_tag: None,
            },
            SignalPoint {
                name: "cv.y1.ist".to_string(),
                signal_type: SignalType::F64,
                address: "ns=2;s=Y1_IST".to_string(),
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
        cfg.runtime.cycle_ms = 1_000;
        cfg
    }

    #[derive(Default)]
    struct LoggingTestLogic {
        params: HashMap<String, LogicParamValue>,
        pending_logs: Vec<LogicUserLog>,
    }

    impl ControlLogic for LoggingTestLogic {
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
        adapter.seed_node("ns=2;s=Y1_SOLL", SignalValue::F64(10.0));
        adapter.seed_node("ns=2;s=Y1_IST", SignalValue::F64(0.0));

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
            build_logic(&base_config(), storage_dir.path()).expect("build logic"),
            None,
            storage,
            log_file,
        )
        .await
        .expect("runtime core");

        core.tick_once().await.expect("tick");

        let written = adapter.read("ns=2;s=U1_WRITE").await.expect("read output");
        assert_eq!(written, SignalValue::F64(1.0));
    }

    #[tokio::test]
    async fn fault_state_machine_transitions_hold_alarm_off() {
        let mut cfg = base_config();
        cfg.alarms.alarm_timeout_ms = 40;
        cfg.alarms.off_timeout_ms = 80;

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-fault"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=Y1_SOLL", SignalValue::F64(1.0));
        adapter.seed_node("ns=2;s=Y1_IST", SignalValue::F64(0.0));

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
            build_logic(&base_config(), storage_dir.path()).expect("build logic"),
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
        adapter.seed_node("ns=2;s=Y1_SOLL", SignalValue::F64(2.0));
        adapter.seed_node("ns=2;s=Y1_IST", SignalValue::F64(1.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let log_path = storage_dir.path().join("runtime.log");
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("log");

        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            build_logic(&base_config(), storage_dir.path()).expect("logic"),
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
        assert!(content.contains("cv.y1.soll"));
        assert!(content.contains("mv.u1.write"));
    }

    #[tokio::test]
    async fn runtime_stores_online_params_and_user_log_values_as_plot_samples() {
        let mut cfg = base_config();
        cfg.logic
            .params
            .insert("custom_gain".to_string(), LogicParamValue::F64(1.5));
        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-plot-extra"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=Y1_SOLL", SignalValue::F64(2.0));
        adapter.seed_node("ns=2;s=Y1_IST", SignalValue::F64(1.0));

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
