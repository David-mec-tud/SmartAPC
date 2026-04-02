use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use tokio::{
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

use crate::{
    communication::{HealthStatus, ProtocolAdapter, opcua::OpcUaClientAdapter},
    logic::{ControlLogic, MappingLogic},
    model::{ModelModule, NoopModel},
    project::ProjectService,
    project_config::{MappingRule, ProjectConfig, WatchdogConfig},
    storage::{CycleMetric, SignalSample, SqliteStorage},
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
}

impl RuntimeCore {
    async fn new(
        config: ProjectConfig,
        protocol: Arc<dyn ProtocolAdapter>,
        mut watchdog: Box<dyn WatchdogModule>,
        mut logic: Box<dyn ControlLogic>,
        model: Option<Box<dyn ModelModule>>,
        storage: SqliteStorage,
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

    fn current_params(&self) -> HashMap<String, f64> {
        self.logic.current_params()
    }

    fn set_params(&mut self, updates: HashMap<String, f64>) {
        self.logic.set_params(updates);
    }

    fn read_plot_samples(&self, limit: usize) -> Result<Vec<SignalSample>> {
        self.storage.read_recent_samples(limit)
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

        let (inputs, input_ok) = match self.read_inputs().await {
            Ok(inputs) => {
                self.storage.store_signal_map("input", &inputs)?;
                self.last_inputs = inputs.clone();
                (inputs, true)
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed reading inputs; entering fault path");
                (self.last_inputs.clone(), false)
            }
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

        match self.state {
            RuntimeState::Running => {
                let mut outputs = self.logic.step(&inputs, self.state)?;
                apply_output_limits(&self.config, &mut outputs);
                self.write_outputs(&outputs).await?;
                self.storage.store_signal_map("output", &outputs)?;
                self.last_outputs = outputs;
            }
            RuntimeState::Hold | RuntimeState::Alarm => {
                if !self.last_outputs.is_empty() {
                    self.write_outputs(&self.last_outputs).await?;
                    self.storage
                        .store_signal_map("output", &self.last_outputs)?;
                }
            }
            RuntimeState::Off => {
                // Off-state explicitly avoids writing outputs.
            }
        }

        let duration = cycle_start.elapsed();
        self.update_metrics(cycle_start, duration)?;
        self.total_cycles += 1;

        Ok(())
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
        let logic = build_logic(&config.logic.mapping_rules, config.logic.params.clone());
        let model = build_model(config.model.is_some());

        let db_path = service
            .project_dir(project_name)
            .join("data")
            .join("runtime.sqlite");
        let storage = SqliteStorage::new(db_path)?;

        let core = Arc::new(Mutex::new(
            RuntimeCore::new(config.clone(), adapter, watchdog, logic, model, storage).await?,
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

    pub async fn set_params(&self, updates: HashMap<String, f64>) -> Result<HashMap<String, f64>> {
        let guard = self.handle.lock().await;
        let handle = guard
            .as_ref()
            .ok_or_else(|| anyhow!("runtime not running"))?;

        let mut core = handle.core.lock().await;
        core.set_params(updates);
        Ok(core.current_params())
    }

    pub async fn get_params(&self) -> Result<HashMap<String, f64>> {
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
            stale_timeout_ms,
        } => Ok(Box::new(CounterMirrorWatchdog::new(
            read_node.clone(),
            write_node.clone(),
            stale_timeout_or_default(*stale_timeout_ms)?,
        ))),
    }
}

fn build_logic(rules: &[MappingRule], params: HashMap<String, f64>) -> Box<dyn ControlLogic> {
    Box::new(MappingLogic::new(rules.to_vec(), params))
}

fn build_model(has_model_config: bool) -> Option<Box<dyn ModelModule>> {
    if has_model_config {
        Some(Box::new(NoopModel::default()))
    } else {
        None
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
    use std::{collections::HashMap, time::Duration};

    use tempfile::tempdir;

    use crate::{
        communication::opcua::OpcUaClientAdapter,
        project_config::{AlarmConfig, LogicConfig, MappingRule, SignalPoint, WatchdogConfig},
    };

    use super::*;

    fn base_config() -> ProjectConfig {
        let mut cfg = ProjectConfig::new("test_project");
        cfg.communication.endpoint_url = "mock://runtime-tests".to_string();
        cfg.io.inputs = vec![SignalPoint {
            name: "pv".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=PV".to_string(),
            opc_tag: None,
        }];
        cfg.io.outputs = vec![SignalPoint {
            name: "mv".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=MV".to_string(),
            opc_tag: None,
        }];
        cfg.logic = LogicConfig {
            kind: "mapping".to_string(),
            mapping_rules: vec![MappingRule {
                input: "pv".to_string(),
                output: "mv".to_string(),
                gain: 2.0,
                offset: 0.0,
                invert_bool: false,
            }],
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

    #[tokio::test]
    async fn limits_are_applied() {
        let mut cfg = base_config();
        cfg.limits.values.insert(
            "mv".to_string(),
            crate::project_config::NumericLimit {
                min: -1.0,
                max: 1.0,
                opc_tag: None,
            },
        );

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-limits"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed nodes");
        adapter.seed_node("ns=2;s=PV", SignalValue::F64(10.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            build_logic(
                &[MappingRule {
                    input: "pv".to_string(),
                    output: "mv".to_string(),
                    gain: 2.0,
                    offset: 0.0,
                    invert_bool: false,
                }],
                HashMap::new(),
            ),
            None,
            storage,
        )
        .await
        .expect("runtime core");

        core.tick_once().await.expect("tick");

        let written = adapter.read("ns=2;s=MV").await.expect("read output");
        assert_eq!(written, SignalValue::F64(1.0));
    }

    #[tokio::test]
    async fn fault_state_machine_transitions_hold_alarm_off() {
        let mut cfg = base_config();
        cfg.alarms.alarm_timeout_ms = 10;
        cfg.alarms.off_timeout_ms = 20;

        let adapter = Arc::new(OpcUaClientAdapter::new("mock://runtime-fault"));
        seed_protocol_nodes(adapter.as_ref(), &cfg).expect("seed");
        adapter.seed_node("ns=2;s=PV", SignalValue::F64(1.0));

        let storage_dir = tempdir().expect("tmp");
        let storage = SqliteStorage::new(storage_dir.path().join("runtime.sqlite")).expect("db");
        let mut core = RuntimeCore::new(
            cfg,
            adapter.clone(),
            Box::new(NoneWatchdog),
            build_logic(
                &[MappingRule {
                    input: "pv".to_string(),
                    output: "mv".to_string(),
                    gain: 1.0,
                    offset: 0.0,
                    invert_bool: false,
                }],
                HashMap::new(),
            ),
            None,
            storage,
        )
        .await
        .expect("runtime core");

        adapter.set_force_fault(true);

        core.tick_once().await.expect("hold");
        assert_eq!(core.state, RuntimeState::Hold);

        tokio::time::sleep(Duration::from_millis(12)).await;
        core.tick_once().await.expect("alarm");
        assert_eq!(core.state, RuntimeState::Alarm);

        tokio::time::sleep(Duration::from_millis(12)).await;
        core.tick_once().await.expect("off");
        assert_eq!(core.state, RuntimeState::Off);
    }
}
