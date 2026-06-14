use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    linear_mpc::linear_mpc_default_params,
    types::{SignalType, SignalValue},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub communication: CommunicationConfig,
    pub watchdog: WatchdogConfig,
    #[serde(default)]
    pub variables: VariableCatalogConfig,
    #[serde(default)]
    pub io_definition: IoDefinitionConfig,
    pub io: IoConfig,
    pub limits: LimitsConfig,
    pub logic: LogicConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelConfig>,
    #[serde(default)]
    pub simulation: SimulationConfig,
    #[serde(default)]
    pub optimization: ParameterOptimizationConfig,
    #[serde(default)]
    pub online_optimization: OnlineOptimizationConfig,
    pub runtime: RuntimeConfig,
    pub alarms: AlarmConfig,
    pub deployment: DeploymentConfig,
    #[serde(default)]
    pub plots: PlotConfig,
}

impl ProjectConfig {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            deployment: DeploymentConfig::default_for_project(&name),
            name,
            communication: CommunicationConfig::default(),
            watchdog: WatchdogConfig::None,
            variables: VariableCatalogConfig::default(),
            io_definition: IoDefinitionConfig::default(),
            io: IoConfig::default(),
            limits: LimitsConfig::default(),
            logic: LogicConfig::default(),
            model: None,
            simulation: SimulationConfig::default(),
            optimization: ParameterOptimizationConfig::default(),
            online_optimization: OnlineOptimizationConfig::default(),
            runtime: RuntimeConfig::default(),
            alarms: AlarmConfig::default(),
            plots: PlotConfig::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.communication.validate()?;
        if self.io.cycle_ms == 0 {
            bail!("io.cycle_ms must be > 0");
        }
        if self.io.cycle_ms % self.communication.cycle_ms != 0 {
            bail!("io.cycle_ms must be a multiple of communication.cycle_ms");
        }
        self.variables.validate()?;
        self.io_definition.validate(&self.variables)?;
        self.io.validate()?;
        self.limits.validate()?;
        if let Some(model) = &self.model {
            model.validate(&self.variables)?;
        }
        self.logic.validate(&self.io)?;
        if self.logic.kind == LogicKind::LinearMpc {
            self.validate_linear_mpc()?;
        }
        self.simulation
            .validate(self.model.as_ref(), &self.variables)?;
        self.optimization.validate(
            self.model.as_ref(),
            &self.variables,
            &self.active_controller_f64_param_defaults(),
        )?;
        self.online_optimization
            .validate(&self.variables, &self.active_f64_online_param_defaults())?;
        self.plots.validate()?;

        if self.logic.cycle_ms == 0 {
            bail!("logic.cycle_ms must be > 0");
        }

        if self.alarms.alarm_timeout_ms == 0 {
            bail!("alarms.alarm_timeout_ms must be > 0");
        }

        if self.alarms.off_timeout_ms <= self.alarms.alarm_timeout_ms {
            bail!("alarms.off_timeout_ms must be > alarms.alarm_timeout_ms");
        }

        if let WatchdogConfig::CounterMirror {
            read_node,
            write_node,
            stale_timeout_ms,
        } = &self.watchdog
        {
            if read_node.trim().is_empty() || write_node.trim().is_empty() {
                bail!("Counter-mirror watchdog requires read_node and write_node");
            }
            if *stale_timeout_ms == 0 {
                bail!("Watchdog stale_timeout_ms must be greater than 0");
            }
        }

        Ok(())
    }

    pub fn normalize_plot_config(&mut self) {
        self.plots.window_sec = self
            .plots
            .window_sec
            .clamp(MIN_PLOT_WINDOW_SEC, MAX_PLOT_WINDOW_SEC);
        self.plots.future_window_sec = self
            .plots
            .future_window_sec
            .clamp(MIN_PLOT_FUTURE_WINDOW_SEC, MAX_PLOT_FUTURE_WINDOW_SEC);
        self.plots.mv_order = normalize_named_order(&self.plots.mv_order, &self.variables.mvs);
        self.plots.cv_order = normalize_named_order(&self.plots.cv_order, &self.variables.cvs);
        self.plots.dv_order = normalize_named_order(&self.plots.dv_order, &self.variables.dvs);

        let mut next_custom = Vec::new();
        for (index, plot) in self.plots.custom_plots.iter().enumerate() {
            let title = if plot.title.trim().is_empty() {
                format!("Custom Plot {}", index + 1)
            } else {
                plot.title.trim().to_string()
            };

            let traces = normalize_custom_traces(&plot.traces);
            if traces.is_empty() {
                continue;
            }

            next_custom.push(CustomPlotConfig { title, traces });
        }
        self.plots.custom_plots = next_custom;
    }

    pub fn normalize_model_config(&mut self) {
        self.model = self
            .model
            .take()
            .and_then(|model| model.normalized_for(&self.variables));
    }

    pub fn normalize_simulation_config(&mut self) {
        let simulation = std::mem::take(&mut self.simulation);
        self.simulation = simulation.normalized_for(&self.variables, self.model.as_ref());
    }

    pub fn normalize_optimization_config(&mut self) {
        let optimization = std::mem::take(&mut self.optimization);
        self.optimization = optimization.normalized_for(
            &self.variables,
            self.model.as_ref(),
            &self.simulation,
            &self.active_controller_f64_param_defaults(),
        );
    }

    pub fn normalize_online_optimization_config(&mut self) {
        let config = std::mem::take(&mut self.online_optimization);
        self.online_optimization = config.normalized_for(
            &self.variables,
            &self.active_f64_online_param_defaults(),
            &self.simulation,
        );
    }

    pub fn rebuild_runtime_io_from_definition(&mut self) -> Result<()> {
        let (mut io, limits) = self
            .io_definition
            .to_runtime_io_and_limits(&self.variables)?;
        io.cycle_ms = self.io.cycle_ms;
        self.io = io;
        self.limits = limits;
        Ok(())
    }

    pub fn build_simulation_io_and_limits(&self) -> Result<(IoConfig, LimitsConfig)> {
        if !self.variables.has_any() {
            bail!("Simulation requires configured MV, CV, or DV names");
        }
        if !self.io_definition.is_complete_for(&self.variables) {
            bail!("Simulation requires a complete I/O definition");
        }

        let io = self.build_simulation_logic_io()?;
        let mut limits = HashMap::new();

        for name in &self.variables.mvs {
            let row = self
                .io_definition
                .mvs
                .iter()
                .find(|row| row.name == *name)
                .expect("checked by is_complete_for");

            let (Some(ll), Some(hl)) = (row.ll, row.hl) else {
                bail!("Simulation requires numeric MV limits for '{}'", name);
            };
            limits.insert(
                format!("mv.{}.write", name),
                NumericLimit {
                    min: ll,
                    max: hl,
                    opc_tag: None,
                },
            );
        }

        Ok((io, LimitsConfig { values: limits }))
    }

    pub fn validate_for_simulation(&self) -> Result<()> {
        let Some(model) = self.model.as_ref() else {
            bail!("Simulation requires a configured model");
        };
        model.validate(&self.variables)?;
        if model.linear_state_space().is_none() {
            bail!("Simulation requires a linear state-space model");
        }

        let (io, limits) = self.build_simulation_io_and_limits()?;
        self.logic.validate(&io)?;
        if self.logic.kind == LogicKind::LinearMpc {
            if let Some(err) = self.linear_mpc_config_error(&io) {
                bail!("{err}");
            }
        }
        limits.validate()?;
        Ok(())
    }

    pub(crate) fn build_simulation_logic_io(&self) -> Result<IoConfig> {
        let has_runtime_contract = !self.io.inputs.is_empty() || !self.io.outputs.is_empty();
        if has_runtime_contract {
            let configured = if self.logic.kind == LogicKind::LinearMpc {
                self.linear_mpc_is_configured_for(&self.io)
            } else {
                self.logic.is_configured_for(&self.io)
            };
            if configured {
                return Ok(self.io.clone());
            }
            bail!(
                "The selected control strategy is not configured for the current runtime I/O definition"
            );
        }

        match self.logic.kind {
            LogicKind::PController => Ok(self.build_minimal_p_controller_simulation_io()),
            LogicKind::LinearMpc => Ok(self.build_minimal_linear_mpc_simulation_io()),
            LogicKind::CustomRust => self.build_minimal_custom_rust_simulation_io(),
        }
    }

    fn build_minimal_p_controller_simulation_io(&self) -> IoConfig {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut input_names = HashSet::new();
        let mut output_names = HashSet::new();

        for pair in &self.logic.p_controller.pairs {
            let setpoint = format!("cv.{}.setpoint", pair.cv);
            if input_names.insert(setpoint.clone()) {
                inputs.push(simulation_signal_point(setpoint, SignalType::F64));
            }

            let process_value = format!("cv.{}.process_value", pair.cv);
            if input_names.insert(process_value.clone()) {
                inputs.push(simulation_signal_point(process_value, SignalType::F64));
            }

            let write = format!("mv.{}.write", pair.mv);
            if output_names.insert(write.clone()) {
                outputs.push(simulation_signal_point(write, SignalType::F64));
            }
        }

        IoConfig {
            inputs,
            outputs,
            ..IoConfig::default()
        }
    }

    fn build_minimal_linear_mpc_simulation_io(&self) -> IoConfig {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        for mv in &self.variables.mvs {
            inputs.push(simulation_signal_point(
                format!("mv.{mv}.read"),
                SignalType::F64,
            ));
            outputs.push(simulation_signal_point(
                format!("mv.{mv}.write"),
                SignalType::F64,
            ));
        }

        for cv in &self.variables.cvs {
            inputs.push(simulation_signal_point(
                format!("cv.{cv}.setpoint"),
                SignalType::F64,
            ));
            inputs.push(simulation_signal_point(
                format!("cv.{cv}.process_value"),
                SignalType::F64,
            ));
        }

        for dv in &self.variables.dvs {
            inputs.push(simulation_signal_point(
                format!("dv.{dv}.read"),
                SignalType::F64,
            ));
        }

        IoConfig {
            inputs,
            outputs,
            ..IoConfig::default()
        }
    }

    fn build_minimal_custom_rust_simulation_io(&self) -> Result<IoConfig> {
        if !self.logic.custom_rust.compatible {
            bail!("Custom Rust strategy is not marked as compatible");
        }
        if self.logic.custom_rust.required_outputs.is_empty() {
            bail!("Custom Rust strategy has no required outputs");
        }

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut input_names = HashSet::new();
        let mut output_names = HashSet::new();

        for name in &self.logic.custom_rust.required_inputs {
            let signal_name = name.trim();
            if signal_name.is_empty() || !input_names.insert(signal_name.to_string()) {
                continue;
            }
            let signal_type = self
                .io
                .inputs
                .iter()
                .find(|point| point.name == signal_name)
                .map(|point| point.signal_type)
                .unwrap_or_else(|| infer_simulation_signal_type(signal_name));
            inputs.push(simulation_signal_point(
                signal_name.to_string(),
                signal_type,
            ));
        }

        for name in &self.logic.custom_rust.required_outputs {
            let signal_name = name.trim();
            if signal_name.is_empty() || !output_names.insert(signal_name.to_string()) {
                continue;
            }
            let signal_type = self
                .io
                .outputs
                .iter()
                .find(|point| point.name == signal_name)
                .map(|point| point.signal_type)
                .unwrap_or_else(|| infer_simulation_signal_type(signal_name));
            outputs.push(simulation_signal_point(
                signal_name.to_string(),
                signal_type,
            ));
        }

        Ok(IoConfig {
            inputs,
            outputs,
            ..IoConfig::default()
        })
    }

    pub fn sync_active_online_params(&mut self) {
        let defaults = self.active_online_param_defaults();
        let mut next = HashMap::new();

        for (key, default_value) in defaults {
            if let Some(existing) = self.logic.params.get(&key) {
                if existing.matches_type(default_value.param_type()) {
                    next.insert(key, existing.clone());
                    continue;
                }
            }
            next.insert(key, default_value);
        }

        self.logic.params = next;
    }

    fn active_online_param_defaults(&self) -> HashMap<String, LogicParamValue> {
        let mut defaults = HashMap::new();

        match self.logic.kind {
            LogicKind::PController => {
                for pair in &self.logic.p_controller.pairs {
                    defaults.insert(
                        format!("p.{}.{}.kp", pair.cv, pair.mv),
                        LogicParamValue::F64(pair.kp),
                    );
                    defaults.insert(
                        format!("p.{}.{}.bias", pair.cv, pair.mv),
                        LogicParamValue::F64(pair.bias),
                    );
                }
            }
            LogicKind::LinearMpc => {
                if let Some(model) = self
                    .model
                    .as_ref()
                    .and_then(ModelConfig::linear_state_space)
                {
                    defaults.extend(linear_mpc_default_params(
                        model,
                        &self.variables,
                        &self.static_cv_band_defaults(),
                    ));
                }
            }
            LogicKind::CustomRust => {
                for entry in &self.logic.custom_rust.parameter_schema {
                    defaults.insert(
                        entry.name.clone(),
                        entry
                            .default
                            .clone()
                            .unwrap_or_else(|| default_logic_param_value(entry.param_type)),
                    );
                }
            }
        }

        for mv in &self.io_definition.mvs {
            if mv.hl_opc.as_deref().unwrap_or("").trim().is_empty() {
                if let Some(value) = mv.hl {
                    defaults.insert(format!("io.mv.{}.hl", mv.name), LogicParamValue::F64(value));
                }
            }
            if mv.ll_opc.as_deref().unwrap_or("").trim().is_empty() {
                if let Some(value) = mv.ll {
                    defaults.insert(format!("io.mv.{}.ll", mv.name), LogicParamValue::F64(value));
                }
            }
        }

        for cv in &self.io_definition.cvs {
            if cv.deadband_opc.as_deref().unwrap_or("").trim().is_empty() {
                if let Some(value) = cv.deadband_value {
                    defaults.insert(
                        format!("io.cv.{}.deadband", cv.name),
                        LogicParamValue::F64(value),
                    );
                }
            }
        }

        defaults
    }

    pub fn active_controller_f64_param_defaults(&self) -> HashMap<String, f64> {
        self.active_online_param_defaults()
            .into_iter()
            .filter_map(|(key, default_value)| {
                if key.starts_with("io.") {
                    return None;
                }
                let value = self
                    .logic
                    .params
                    .get(&key)
                    .filter(|existing| existing.matches_type(default_value.param_type()))
                    .unwrap_or(&default_value);
                match value {
                    LogicParamValue::F64(value) => Some((key, *value)),
                    LogicParamValue::Bool(_) => None,
                }
            })
            .collect()
    }

    pub fn active_f64_online_param_defaults(&self) -> HashMap<String, f64> {
        self.active_online_param_defaults()
            .into_iter()
            .filter_map(|(key, default_value)| {
                let value = self
                    .logic
                    .params
                    .get(&key)
                    .filter(|existing| existing.matches_type(default_value.param_type()))
                    .unwrap_or(&default_value);
                match value {
                    LogicParamValue::F64(value) => Some((key, *value)),
                    LogicParamValue::Bool(_) => None,
                }
            })
            .collect()
    }

    pub fn linear_mpc_is_configured_for(&self, io: &IoConfig) -> bool {
        self.linear_mpc_config_error(io).is_none()
    }

    fn validate_linear_mpc(&self) -> Result<()> {
        if let Some(err) = self.linear_mpc_config_error(&self.io) {
            bail!("{err}");
        }
        Ok(())
    }

    fn linear_mpc_config_error(&self, io: &IoConfig) -> Option<String> {
        let Some(model) = self
            .model
            .as_ref()
            .and_then(ModelConfig::linear_state_space)
        else {
            return Some("Linear MPC requires a linear state-space model".to_string());
        };
        if self.variables.mvs.is_empty() || self.variables.cvs.is_empty() {
            return Some("Linear MPC requires at least one MV and one CV".to_string());
        }

        let expected_u_order: Vec<_> = self
            .variables
            .mvs
            .iter()
            .cloned()
            .chain(self.variables.dvs.iter().cloned())
            .collect();
        if model.u_order != expected_u_order {
            return Some("Linear MPC model u_order must list MVs before DVs".to_string());
        }
        if model.y_order != self.variables.cvs {
            return Some("Linear MPC model y_order must match the CV order".to_string());
        }

        let input_names: HashSet<_> = io.inputs.iter().map(|point| point.name.as_str()).collect();
        let output_names: HashSet<_> = io.outputs.iter().map(|point| point.name.as_str()).collect();

        for cv in &self.variables.cvs {
            let setpoint = format!("cv.{cv}.setpoint");
            let process_value = format!("cv.{cv}.process_value");
            if !input_names.contains(setpoint.as_str()) {
                return Some(format!("Linear MPC requires input '{setpoint}'"));
            }
            if !input_names.contains(process_value.as_str()) {
                return Some(format!("Linear MPC requires input '{process_value}'"));
            }
        }
        for mv in &self.variables.mvs {
            let write = format!("mv.{mv}.write");
            if !output_names.contains(write.as_str()) {
                return Some(format!("Linear MPC requires output '{write}'"));
            }
        }
        for dv in &self.variables.dvs {
            let read = format!("dv.{dv}.read");
            if !input_names.contains(read.as_str()) {
                return Some(format!("Linear MPC requires input '{read}'"));
            }
        }

        None
    }

    pub fn static_cv_band_defaults(&self) -> HashMap<String, f64> {
        let mut values = HashMap::new();
        for cv in &self.io_definition.cvs {
            if cv.deadband_opc.as_deref().unwrap_or("").trim().is_empty() {
                if let Some(value) = cv.deadband_value {
                    values.insert(cv.name.clone(), value);
                }
            }
        }
        values
    }

    pub fn sidebar_tasks(&self) -> Vec<SidebarTask> {
        let legacy_io_present = !self.io.inputs.is_empty() || !self.io.outputs.is_empty();
        let variables_configured = self.variables.has_any() || legacy_io_present;
        let io_definition_configured = if self.variables.has_any() {
            self.io_definition.is_complete_for(&self.variables)
        } else {
            !self.io.inputs.is_empty() && !self.io.outputs.is_empty()
        };
        let logic_configured = if self.logic.kind == LogicKind::LinearMpc {
            self.linear_mpc_is_configured_for(&self.io)
        } else {
            self.logic.is_configured_for(&self.io)
        };

        vec![
            SidebarTask::new(
                "communication",
                "Configure OPC UA Communication",
                !self.communication.endpoint_url.trim().is_empty()
                    && self.communication.cycle_ms > 0,
            ),
            SidebarTask::new(
                "watchdog",
                "Define Watchdog",
                !matches!(self.watchdog, WatchdogConfig::None),
            ),
            SidebarTask::new("io", "Define Inputs and Outputs", variables_configured),
            SidebarTask::new("limits", "Define I/O", io_definition_configured),
            SidebarTask::new("model", "Define Model (optional)", true),
            SidebarTask::new("logic", "Configure Control Strategy", logic_configured),
            SidebarTask::new(
                "kalman_tuning",
                "Kalman Filter Tuning",
                self.kalman_tuning_available(),
            ),
            SidebarTask::new("optimization", "Parameter Optimization", logic_configured),
            SidebarTask::new(
                "online_optimization",
                "Online Parameter Optimization",
                logic_configured,
            ),
            SidebarTask::new("timing", "Control Function Tests", logic_configured),
            SidebarTask::new("simulation", "Simulate Controller (optional)", true),
            SidebarTask::new(
                "deployment",
                "Configure Deployment",
                !self.deployment.service_name.is_empty(),
            ),
        ]
    }

    pub fn kalman_tuning_available(&self) -> bool {
        self.logic.kind == LogicKind::LinearMpc
            && self.linear_mpc_is_configured_for(&self.io)
            && self
                .model
                .as_ref()
                .and_then(ModelConfig::linear_state_space)
                .is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidebarTask {
    pub id: String,
    pub label: String,
    pub configured: bool,
}

impl SidebarTask {
    fn new(id: impl Into<String>, label: impl Into<String>, configured: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            configured,
        }
    }
}

pub const MIN_PLOT_WINDOW_SEC: u32 = 10;
pub const MAX_PLOT_WINDOW_SEC: u32 = 3_600;
pub const MIN_PLOT_FUTURE_WINDOW_SEC: u32 = 0;
pub const MAX_PLOT_FUTURE_WINDOW_SEC: u32 = 3_600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlotConfig {
    #[serde(default = "default_plot_window_sec")]
    pub window_sec: u32,
    #[serde(default = "default_plot_future_window_sec")]
    pub future_window_sec: u32,
    #[serde(default)]
    pub mv_order: Vec<String>,
    #[serde(default)]
    pub cv_order: Vec<String>,
    #[serde(default)]
    pub dv_order: Vec<String>,
    #[serde(default)]
    pub custom_plots: Vec<CustomPlotConfig>,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            window_sec: default_plot_window_sec(),
            future_window_sec: default_plot_future_window_sec(),
            mv_order: Vec::new(),
            cv_order: Vec::new(),
            dv_order: Vec::new(),
            custom_plots: Vec::new(),
        }
    }
}

impl PlotConfig {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPlotConfig {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub traces: Vec<CustomPlotTraceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CustomPlotTraceConfig {
    pub direction: String,
    pub signal: String,
}

const fn default_plot_window_sec() -> u32 {
    180
}

const fn default_plot_future_window_sec() -> u32 {
    0
}

fn normalize_named_order(order: &[String], defaults: &[String]) -> Vec<String> {
    let allowed: HashSet<&str> = defaults.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for raw in order {
        let name = raw.trim();
        if name.is_empty() || !allowed.contains(name) {
            continue;
        }
        if seen.insert(name.to_string()) {
            out.push(name.to_string());
        }
    }

    for name in defaults {
        if seen.insert(name.clone()) {
            out.push(name.clone());
        }
    }

    out
}

fn normalize_custom_traces(traces: &[CustomPlotTraceConfig]) -> Vec<CustomPlotTraceConfig> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for trace in traces {
        let direction = trace.direction.trim();
        let signal = trace.signal.trim();
        if direction.is_empty() || signal.is_empty() {
            continue;
        }
        let key = format!("{direction}::{signal}");
        if seen.insert(key) {
            out.push(CustomPlotTraceConfig {
                direction: direction.to_string(),
                signal: signal.to_string(),
            });
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunicationConfig {
    pub protocol: ProtocolKind,
    pub endpoint_url: String,
    #[serde(default = "default_cycle_ms")]
    pub cycle_ms: u64,
}

impl Default for CommunicationConfig {
    fn default() -> Self {
        Self {
            protocol: ProtocolKind::OpcUa,
            endpoint_url: "opc.tcp://127.0.0.1:4840".to_string(),
            cycle_ms: default_cycle_ms(),
        }
    }
}

impl CommunicationConfig {
    fn validate(&self) -> Result<()> {
        if self.endpoint_url.trim().is_empty() {
            bail!("communication.endpoint_url must not be empty");
        }
        if self.cycle_ms == 0 {
            bail!("communication.cycle_ms must be > 0");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    OpcUa,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchdogConfig {
    None,
    CounterMirror {
        read_node: String,
        write_node: String,
        #[serde(default = "default_watchdog_stale_timeout_ms")]
        stale_timeout_ms: u64,
    },
}

const fn default_watchdog_stale_timeout_ms() -> u64 {
    2_000
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VariableCatalogConfig {
    #[serde(default)]
    pub mvs: Vec<String>,
    #[serde(default)]
    pub cvs: Vec<String>,
    #[serde(default)]
    pub dvs: Vec<String>,
}

impl VariableCatalogConfig {
    pub fn has_any(&self) -> bool {
        !(self.mvs.is_empty() && self.cvs.is_empty() && self.dvs.is_empty())
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        validate_variable_group("mv", &self.mvs, &mut seen)?;
        validate_variable_group("cv", &self.cvs, &mut seen)?;
        validate_variable_group("dv", &self.dvs, &mut seen)?;
        Ok(())
    }

    fn has_mv(&self, name: &str) -> bool {
        self.mvs.iter().any(|n| n == name)
    }

    fn has_cv(&self, name: &str) -> bool {
        self.cvs.iter().any(|n| n == name)
    }

    fn has_dv(&self, name: &str) -> bool {
        self.dvs.iter().any(|n| n == name)
    }
}

fn validate_variable_group(kind: &str, names: &[String], seen: &mut HashSet<String>) -> Result<()> {
    for raw_name in names {
        let name = raw_name.trim();
        if name.is_empty() {
            bail!("{} name must not be empty", kind);
        }
        if !seen.insert(name.to_string()) {
            bail!("duplicate variable name '{}': names must be unique", name);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IoDefinitionConfig {
    #[serde(default)]
    pub mvs: Vec<MvIoDefinition>,
    #[serde(default)]
    pub cvs: Vec<CvIoDefinition>,
    #[serde(default)]
    pub dvs: Vec<DvIoDefinition>,
}

impl IoDefinitionConfig {
    pub(crate) fn validate(&self, vars: &VariableCatalogConfig) -> Result<()> {
        validate_named_entries("mv", &self.mvs, |row| &row.name, |name| vars.has_mv(name))?;
        validate_named_entries("cv", &self.cvs, |row| &row.name, |name| vars.has_cv(name))?;
        validate_named_entries("dv", &self.dvs, |row| &row.name, |name| vars.has_dv(name))?;

        for row in &self.mvs {
            if let (Some(ll), Some(hl)) = (row.ll, row.hl) {
                if ll > hl {
                    bail!(
                        "MV '{}' has a lower limit greater than its upper limit",
                        row.name
                    );
                }
            }
        }

        Ok(())
    }

    pub fn is_complete_for(&self, vars: &VariableCatalogConfig) -> bool {
        for name in &vars.mvs {
            let Some(row) = self.mvs.iter().find(|row| row.name == *name) else {
                return false;
            };
            if let (Some(ll), Some(hl)) = (row.ll, row.hl) {
                if ll > hl {
                    return false;
                }
            }
        }

        for name in &vars.cvs {
            if self.cvs.iter().find(|row| row.name == *name).is_none() {
                return false;
            }
        }

        for name in &vars.dvs {
            if self.dvs.iter().find(|row| row.name == *name).is_none() {
                return false;
            }
        }

        true
    }

    pub fn to_runtime_io_and_limits(
        &self,
        vars: &VariableCatalogConfig,
    ) -> Result<(IoConfig, LimitsConfig)> {
        if !self.is_complete_for(vars) {
            bail!("I/O definition is incomplete for the configured variables");
        }

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut limits = HashMap::new();

        for name in &vars.mvs {
            let row = self
                .mvs
                .iter()
                .find(|row| row.name == *name)
                .expect("checked by is_complete_for");

            if let Some(addr) = optional_trimmed(&row.mv_write) {
                outputs.push(SignalPoint {
                    name: format!("mv.{}.write", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }
            if let Some(addr) = optional_trimmed(&row.mv_read) {
                inputs.push(SignalPoint {
                    name: format!("mv.{}.read", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }

            if let Some(enable) = optional_trimmed(row.enable.as_deref().unwrap_or("")) {
                inputs.push(SignalPoint {
                    name: format!("mv.{}.enable", name),
                    signal_type: SignalType::Bool,
                    address: enable,
                    opc_tag: None,
                });
            }

            if let Some(addr) = optional_trimmed(row.hl_opc.as_deref().unwrap_or("")) {
                inputs.push(SignalPoint {
                    name: format!("mv.{}.hl", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }
            if let Some(addr) = optional_trimmed(row.ll_opc.as_deref().unwrap_or("")) {
                inputs.push(SignalPoint {
                    name: format!("mv.{}.ll", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }

            if let (Some(ll), Some(hl)) = (row.ll, row.hl) {
                if !row.mv_write.trim().is_empty() {
                    limits.insert(
                        format!("mv.{}.write", name),
                        NumericLimit {
                            min: ll,
                            max: hl,
                            opc_tag: None,
                        },
                    );
                }
            }
        }

        for name in &vars.cvs {
            let row = self
                .cvs
                .iter()
                .find(|row| row.name == *name)
                .expect("checked by is_complete_for");

            if let Some(addr) = optional_trimmed(&row.setpoint) {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.setpoint", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }
            if let Some(addr) = optional_trimmed(&row.process_value) {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.process_value", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }

            if let Some(deadband_opc) = optional_trimmed(row.deadband_opc.as_deref().unwrap_or(""))
            {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.deadband", name),
                    signal_type: SignalType::F64,
                    address: deadband_opc,
                    opc_tag: None,
                });
            }

            if let Some(enable) = optional_trimmed(row.enable.as_deref().unwrap_or("")) {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.enable", name),
                    signal_type: SignalType::Bool,
                    address: enable,
                    opc_tag: None,
                });
            }
        }

        for name in &vars.dvs {
            let row = self
                .dvs
                .iter()
                .find(|row| row.name == *name)
                .expect("checked by is_complete_for");

            if let Some(addr) = optional_trimmed(&row.read) {
                inputs.push(SignalPoint {
                    name: format!("dv.{}.read", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }

            if let Some(enable) = optional_trimmed(row.enable.as_deref().unwrap_or("")) {
                inputs.push(SignalPoint {
                    name: format!("dv.{}.enable", name),
                    signal_type: SignalType::Bool,
                    address: enable,
                    opc_tag: None,
                });
            }
        }

        Ok((
            IoConfig {
                inputs,
                outputs,
                ..IoConfig::default()
            },
            LimitsConfig { values: limits },
        ))
    }
}

fn validate_named_entries<T, F, G>(
    kind: &str,
    rows: &[T],
    get_name: F,
    name_exists: G,
) -> Result<()>
where
    F: Fn(&T) -> &String,
    G: Fn(&str) -> bool,
{
    let mut seen = HashSet::new();
    for row in rows {
        let name = get_name(row).trim();
        if name.is_empty() {
            bail!("{} io_definition entry requires non-empty name", kind);
        }
        if !seen.insert(name.to_string()) {
            bail!("duplicate {} io_definition name '{}'", kind, name);
        }
        if !name_exists(name) {
            bail!(
                "{} io_definition references unknown variable '{}'",
                kind,
                name
            );
        }
    }
    Ok(())
}

fn optional_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn simulation_signal_point(name: String, signal_type: SignalType) -> SignalPoint {
    SignalPoint {
        name: name.clone(),
        signal_type,
        address: format!("sim://{}", name.replace('.', "/")),
        opc_tag: None,
    }
}

fn infer_simulation_signal_type(name: &str) -> SignalType {
    if name.ends_with(".enable") {
        SignalType::Bool
    } else {
        SignalType::F64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MvIoDefinition {
    pub name: String,
    #[serde(default)]
    pub mv_write: String,
    #[serde(default)]
    pub mv_read: String,
    #[serde(default)]
    pub hl_opc: Option<String>,
    #[serde(default)]
    pub ll_opc: Option<String>,
    #[serde(default)]
    pub hl: Option<f64>,
    #[serde(default)]
    pub ll: Option<f64>,
    #[serde(default)]
    pub enable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CvIoDefinition {
    pub name: String,
    #[serde(default)]
    pub setpoint: String,
    #[serde(default)]
    pub process_value: String,
    #[serde(default)]
    pub deadband_opc: Option<String>,
    #[serde(default)]
    pub deadband_value: Option<f64>,
    #[serde(default)]
    pub enable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DvIoDefinition {
    pub name: String,
    #[serde(default)]
    pub read: String,
    #[serde(default)]
    pub enable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoConfig {
    #[serde(default = "default_cycle_ms")]
    pub cycle_ms: u64,
    #[serde(default)]
    pub inputs: Vec<SignalPoint>,
    #[serde(default)]
    pub outputs: Vec<SignalPoint>,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            cycle_ms: default_cycle_ms(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

impl IoConfig {
    fn validate(&self) -> Result<()> {
        let mut names = HashSet::new();
        for s in self.inputs.iter().chain(self.outputs.iter()) {
            if s.name.trim().is_empty() {
                bail!("io signal name must not be empty");
            }
            if !names.insert(s.name.clone()) {
                bail!("duplicate signal name '{}': names must be unique", s.name);
            }
            if s.address.trim().is_empty() {
                bail!("signal '{}' requires non-empty address", s.name);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalPoint {
    pub name: String,
    pub signal_type: SignalType,
    pub address: String,
    #[serde(default)]
    pub opc_tag: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LimitsConfig {
    #[serde(default)]
    pub values: HashMap<String, NumericLimit>,
}

impl LimitsConfig {
    fn validate(&self) -> Result<()> {
        for (key, limit) in &self.values {
            if limit.min > limit.max {
                bail!("limit '{}' has min > max", key);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericLimit {
    pub min: f64,
    pub max: f64,
    #[serde(default)]
    pub opc_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicConfig {
    #[serde(default = "default_cycle_ms")]
    pub cycle_ms: u64,
    #[serde(default)]
    pub kind: LogicKind,
    #[serde(default)]
    pub p_controller: PControllerLogicConfig,
    #[serde(default)]
    pub custom_rust: CustomRustLogicConfig,
    #[serde(default)]
    pub params: HashMap<String, LogicParamValue>,
    #[serde(default)]
    pub initial_values: HashMap<String, SignalValue>,
}

impl Default for LogicConfig {
    fn default() -> Self {
        Self {
            cycle_ms: default_cycle_ms(),
            kind: LogicKind::PController,
            p_controller: PControllerLogicConfig::default(),
            custom_rust: CustomRustLogicConfig::default(),
            params: HashMap::new(),
            initial_values: HashMap::new(),
        }
    }
}

impl LogicConfig {
    fn validate(&self, io: &IoConfig) -> Result<()> {
        let input_set: HashSet<_> = io.inputs.iter().map(|s| s.name.as_str()).collect();
        let output_set: HashSet<_> = io.outputs.iter().map(|s| s.name.as_str()).collect();

        self.p_controller.validate(io, &input_set, &output_set)?;
        self.custom_rust.validate(io, &input_set, &output_set)?;

        if self.kind == LogicKind::CustomRust {
            let mut schema_by_name = HashMap::new();
            for entry in &self.custom_rust.parameter_schema {
                if schema_by_name
                    .insert(entry.name.as_str(), entry.param_type)
                    .is_some()
                {
                    bail!("duplicate custom parameter schema entry '{}'", entry.name);
                }
            }

            for (key, value) in &self.params {
                if let Some(param_type) = schema_by_name.get(key.as_str()) {
                    if !value.matches_type(*param_type) {
                        bail!(
                            "logic.params['{}'] has wrong type (expected {:?})",
                            key,
                            param_type
                        );
                    }
                }
            }
        }

        Ok(())
    }

    pub fn is_configured_for(&self, io: &IoConfig) -> bool {
        match self.kind {
            LogicKind::PController => self.p_controller.is_configured_for(io),
            LogicKind::LinearMpc => true,
            LogicKind::CustomRust => self.custom_rust.is_compatible_with(io),
        }
    }
}

fn default_logic_param_value(param_type: LogicParamType) -> LogicParamValue {
    match param_type {
        LogicParamType::F64 => LogicParamValue::F64(0.0),
        LogicParamType::Bool => LogicParamValue::Bool(false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicKind {
    PController,
    LinearMpc,
    CustomRust,
}

impl Default for LogicKind {
    fn default() -> Self {
        Self::PController
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicParamType {
    F64,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LogicParamValue {
    F64(f64),
    Bool(bool),
}

impl LogicParamValue {
    pub fn param_type(&self) -> LogicParamType {
        match self {
            Self::F64(_) => LogicParamType::F64,
            Self::Bool(_) => LogicParamType::Bool,
        }
    }

    pub fn matches_type(&self, param_type: LogicParamType) -> bool {
        self.param_type() == param_type
    }
}

impl From<SignalValue> for LogicParamValue {
    fn from(value: SignalValue) -> Self {
        match value {
            SignalValue::F64(v) => Self::F64(v),
            SignalValue::Bool(v) => Self::Bool(v),
        }
    }
}

impl From<LogicParamValue> for SignalValue {
    fn from(value: LogicParamValue) -> Self {
        match value {
            LogicParamValue::F64(v) => Self::F64(v),
            LogicParamValue::Bool(v) => Self::Bool(v),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PControllerLogicConfig {
    #[serde(default)]
    pub pairs: Vec<PControllerPairConfig>,
}

impl PControllerLogicConfig {
    fn validate(
        &self,
        io: &IoConfig,
        input_set: &HashSet<&str>,
        output_set: &HashSet<&str>,
    ) -> Result<()> {
        let mut seen = HashSet::new();
        for pair in &self.pairs {
            if pair.cv.trim().is_empty() || pair.mv.trim().is_empty() {
                bail!("P controller pair requires a CV and an MV");
            }
            let key = format!("{}->{}", pair.cv.trim(), pair.mv.trim());
            if !seen.insert(key) {
                bail!("duplicate P controller pair '{}->{}'", pair.cv, pair.mv);
            }
            if !pair.kp.is_finite() || !pair.bias.is_finite() {
                bail!(
                    "p_controller pair '{}->{}' has non-finite kp/bias",
                    pair.cv,
                    pair.mv
                );
            }

            if !io.inputs.is_empty() || !io.outputs.is_empty() {
                let setpoint = format!("cv.{}.setpoint", pair.cv);
                let process_value = format!("cv.{}.process_value", pair.cv);
                let mv_write = format!("mv.{}.write", pair.mv);
                if !input_set.contains(setpoint.as_str()) {
                    bail!("P controller pair references missing input '{}'", setpoint);
                }
                if !input_set.contains(process_value.as_str()) {
                    bail!(
                        "P controller pair references missing input '{}'",
                        process_value
                    );
                }
                if !output_set.contains(mv_write.as_str()) {
                    bail!("P controller pair references missing output '{}'", mv_write);
                }
            }
        }
        Ok(())
    }

    pub fn is_configured_for(&self, io: &IoConfig) -> bool {
        if self.pairs.is_empty() {
            return false;
        }
        let inputs: HashSet<_> = io.inputs.iter().map(|s| s.name.as_str()).collect();
        let outputs: HashSet<_> = io.outputs.iter().map(|s| s.name.as_str()).collect();
        self.pairs.iter().all(|pair| {
            let setpoint = format!("cv.{}.setpoint", pair.cv);
            let process_value = format!("cv.{}.process_value", pair.cv);
            let mv_write = format!("mv.{}.write", pair.mv);
            inputs.contains(setpoint.as_str())
                && inputs.contains(process_value.as_str())
                && outputs.contains(mv_write.as_str())
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PControllerPairConfig {
    pub cv: String,
    pub mv: String,
    #[serde(default = "default_p_kp")]
    pub kp: f64,
    #[serde(default)]
    pub bias: f64,
}

const fn default_p_kp() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomRustLogicConfig {
    #[serde(default)]
    pub strategy_name: String,
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub artifact_path: String,
    #[serde(default)]
    pub parameter_schema: Vec<CustomParameterSchemaEntry>,
    #[serde(default)]
    pub required_inputs: Vec<String>,
    #[serde(default)]
    pub required_outputs: Vec<String>,
    #[serde(default)]
    pub compatible: bool,
    #[serde(default)]
    pub last_build_message: String,
}

impl CustomRustLogicConfig {
    fn validate(
        &self,
        io: &IoConfig,
        input_set: &HashSet<&str>,
        output_set: &HashSet<&str>,
    ) -> Result<()> {
        let mut names = HashSet::new();
        for entry in &self.parameter_schema {
            if entry.name.trim().is_empty() {
                bail!("custom parameter schema name must not be empty");
            }
            if !names.insert(entry.name.clone()) {
                bail!("duplicate custom parameter schema '{}'", entry.name);
            }
            if let Some(default) = &entry.default {
                if !default.matches_type(entry.param_type) {
                    bail!("custom parameter '{}' default has wrong type", entry.name);
                }
            }
        }

        if self.compatible {
            for required in &self.required_inputs {
                if !input_set.contains(required.as_str()) {
                    bail!(
                        "custom control strategy requires unknown input '{}'",
                        required
                    );
                }
            }
            for required in &self.required_outputs {
                if !output_set.contains(required.as_str()) {
                    bail!(
                        "custom control strategy requires unknown output '{}'",
                        required
                    );
                }
            }
        }

        if self.compatible {
            if self.artifact_path.trim().is_empty() {
                bail!("custom_rust.artifact_path must not be empty when compatible=true");
            }
            if self.source_path.trim().is_empty() {
                bail!("custom_rust.source_path must not be empty when compatible=true");
            }
            if io.outputs.is_empty() {
                bail!("custom control strategy cannot be compatible without runtime outputs");
            }
        }

        Ok(())
    }

    pub fn is_compatible_with(&self, io: &IoConfig) -> bool {
        if !self.compatible
            || self.source_path.trim().is_empty()
            || self.artifact_path.trim().is_empty()
            || self.required_outputs.is_empty()
        {
            return false;
        }

        let expected_inputs: HashSet<_> = io.inputs.iter().map(|s| s.name.as_str()).collect();
        let expected_outputs: HashSet<_> = io.outputs.iter().map(|s| s.name.as_str()).collect();
        let actual_inputs: HashSet<_> = self.required_inputs.iter().map(|s| s.as_str()).collect();
        let actual_outputs: HashSet<_> = self.required_outputs.iter().map(|s| s.as_str()).collect();

        actual_inputs == expected_inputs && actual_outputs == expected_outputs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomParameterSchemaEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: LogicParamType,
    #[serde(default)]
    pub default: Option<LogicParamValue>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelConfig {
    Noop {
        #[serde(default)]
        config: HashMap<String, serde_json::Value>,
    },
    LinearStateSpace {
        #[serde(flatten)]
        model: LinearStateSpaceModelConfig,
    },
}

impl ModelConfig {
    fn validate(&self, vars: &VariableCatalogConfig) -> Result<()> {
        match self {
            Self::Noop { .. } => Ok(()),
            Self::LinearStateSpace { model } => model.validate(vars),
        }
    }

    fn normalized_for(self, vars: &VariableCatalogConfig) -> Option<Self> {
        match self {
            Self::Noop { .. } => None,
            Self::LinearStateSpace { model } => Some(Self::LinearStateSpace {
                model: model.normalized_for(vars),
            }),
        }
    }

    pub fn linear_state_space(&self) -> Option<&LinearStateSpaceModelConfig> {
        match self {
            Self::LinearStateSpace { model } => Some(model),
            Self::Noop { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinearStateSpaceModelConfig {
    pub state_count: usize,
    #[serde(default)]
    pub u_order: Vec<String>,
    #[serde(default)]
    pub y_order: Vec<String>,
    #[serde(default)]
    pub a: Vec<Vec<f64>>,
    #[serde(default)]
    pub b: Vec<Vec<f64>>,
    #[serde(default)]
    pub c: Vec<Vec<f64>>,
    #[serde(default)]
    pub d: Vec<Vec<f64>>,
}

impl LinearStateSpaceModelConfig {
    pub fn validate(&self, vars: &VariableCatalogConfig) -> Result<()> {
        if self.state_count == 0 {
            bail!("model.linear_state_space.state_count must be > 0");
        }

        let expected_u_order = linear_model_u_order(vars);
        let expected_y_order = linear_model_y_order(vars);

        if self.u_order != expected_u_order {
            bail!("model.linear_state_space.u_order must match MV then DV order");
        }

        if self.y_order != expected_y_order {
            bail!("model.linear_state_space.y_order must match CV order");
        }

        validate_matrix("A", &self.a, self.state_count, self.state_count)?;
        validate_matrix("B", &self.b, self.state_count, self.u_order.len())?;
        validate_matrix("C", &self.c, self.y_order.len(), self.state_count)?;
        validate_matrix("D", &self.d, self.y_order.len(), self.u_order.len())?;

        Ok(())
    }

    fn normalized_for(self, vars: &VariableCatalogConfig) -> Self {
        let new_u_order = linear_model_u_order(vars);
        let new_y_order = linear_model_y_order(vars);
        let old_u_order = self.u_order;
        let old_y_order = self.y_order;
        let state_count = self.state_count;

        Self {
            state_count,
            u_order: new_u_order.clone(),
            y_order: new_y_order.clone(),
            a: resize_matrix_overlap(self.a, state_count, state_count),
            b: remap_matrix_columns(self.b, state_count, &old_u_order, &new_u_order),
            c: remap_matrix_rows(self.c, &old_y_order, &new_y_order, state_count),
            d: remap_matrix_rows_and_columns(
                self.d,
                &old_y_order,
                &new_y_order,
                &old_u_order,
                &new_u_order,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulationSpeedMode {
    #[default]
    RealTime,
    X0_1,
    X0_5,
    X10,
    X20,
    X50,
    X100,
    AsFastAsPossible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterOptimizationConfig {
    #[serde(default)]
    pub parameters: Vec<ParameterOptimizationParamConfig>,
    #[serde(default)]
    pub cv_target_steps: HashMap<String, f64>,
    #[serde(default)]
    pub cv_weights: HashMap<String, f64>,
    #[serde(default)]
    pub cv_noise_std: HashMap<String, f64>,
    #[serde(default)]
    pub mv_noise_std: HashMap<String, f64>,
    #[serde(default)]
    pub metric_weights: ParameterOptimizationMetricWeights,
    #[serde(default = "default_a_scale_min_pct")]
    pub a_scale_min_pct: f64,
    #[serde(default = "default_a_scale_max_pct")]
    pub a_scale_max_pct: f64,
    #[serde(default = "default_b_scale_min_pct")]
    pub b_scale_min_pct: f64,
    #[serde(default = "default_b_scale_max_pct")]
    pub b_scale_max_pct: f64,
    #[serde(default = "default_optimization_scenario_count")]
    pub scenario_count: usize,
    #[serde(default = "default_optimization_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_optimization_initial_samples")]
    pub initial_samples: usize,
    #[serde(default = "default_optimization_convergence_patience")]
    pub convergence_patience: usize,
    #[serde(default = "default_optimization_plot_interval")]
    pub plot_interval: usize,
    #[serde(default = "default_optimization_duration_s")]
    pub simulation_duration_s: f64,
}

impl Default for ParameterOptimizationConfig {
    fn default() -> Self {
        Self {
            parameters: Vec::new(),
            cv_target_steps: HashMap::new(),
            cv_weights: HashMap::new(),
            cv_noise_std: HashMap::new(),
            mv_noise_std: HashMap::new(),
            metric_weights: ParameterOptimizationMetricWeights::default(),
            a_scale_min_pct: default_a_scale_min_pct(),
            a_scale_max_pct: default_a_scale_max_pct(),
            b_scale_min_pct: default_b_scale_min_pct(),
            b_scale_max_pct: default_b_scale_max_pct(),
            scenario_count: default_optimization_scenario_count(),
            max_iterations: default_optimization_max_iterations(),
            initial_samples: default_optimization_initial_samples(),
            convergence_patience: default_optimization_convergence_patience(),
            plot_interval: default_optimization_plot_interval(),
            simulation_duration_s: default_optimization_duration_s(),
        }
    }
}

impl ParameterOptimizationConfig {
    fn normalized_for(
        self,
        vars: &VariableCatalogConfig,
        model: Option<&ModelConfig>,
        simulation: &SimulationConfig,
        active_params: &HashMap<String, f64>,
    ) -> Self {
        let mut existing_by_key: HashMap<_, _> = self
            .parameters
            .into_iter()
            .map(|row| (row.key.clone(), row))
            .collect();

        let mut keys: Vec<_> = active_params.keys().cloned().collect();
        keys.sort();
        let parameters = keys
            .into_iter()
            .map(|key| {
                let default = active_params.get(&key).copied().unwrap_or(0.0);
                existing_by_key
                    .remove(&key)
                    .map(|row| row.normalized_for_default(default))
                    .unwrap_or_else(|| ParameterOptimizationParamConfig::from_default(key, default))
            })
            .collect();

        let cv_target_steps =
            normalize_named_f64_map_with(self.cv_target_steps, &vars.cvs, |name| {
                simulation.cv_setpoints.get(name).copied().unwrap_or(1.0)
            });
        let cv_weights = normalize_named_f64_map(self.cv_weights, &vars.cvs, 1.0);
        let cv_noise_std = normalize_named_f64_map(self.cv_noise_std, &vars.cvs, 0.0);
        let mv_noise_std = normalize_named_f64_map(self.mv_noise_std, &vars.mvs, 0.0);

        let has_model = model.and_then(ModelConfig::linear_state_space).is_some();
        Self {
            parameters,
            cv_target_steps,
            cv_weights,
            cv_noise_std,
            mv_noise_std,
            metric_weights: self.metric_weights.normalized(),
            a_scale_min_pct: finite_or(self.a_scale_min_pct, default_a_scale_min_pct()),
            a_scale_max_pct: finite_or(self.a_scale_max_pct, default_a_scale_max_pct()),
            b_scale_min_pct: finite_or(self.b_scale_min_pct, default_b_scale_min_pct()),
            b_scale_max_pct: finite_or(self.b_scale_max_pct, default_b_scale_max_pct()),
            scenario_count: self.scenario_count.clamp(1, 256),
            max_iterations: self.max_iterations.clamp(1, 10_000),
            initial_samples: self.initial_samples.clamp(1, 1_000),
            convergence_patience: self.convergence_patience.clamp(1, 1_000),
            plot_interval: self.plot_interval.clamp(1, 1_000),
            simulation_duration_s: if has_model {
                finite_or(
                    self.simulation_duration_s,
                    default_optimization_duration_s(),
                )
                .max(1.0)
            } else {
                default_optimization_duration_s()
            },
        }
    }

    fn validate(
        &self,
        model: Option<&ModelConfig>,
        vars: &VariableCatalogConfig,
        active_params: &HashMap<String, f64>,
    ) -> Result<()> {
        validate_named_f64_values(
            "optimization.cv_target_steps",
            &self.cv_target_steps,
            &vars.cvs,
            false,
        )?;
        validate_named_f64_values("optimization.cv_weights", &self.cv_weights, &vars.cvs, true)?;
        validate_named_f64_values(
            "optimization.cv_noise_std",
            &self.cv_noise_std,
            &vars.cvs,
            true,
        )?;
        validate_named_f64_values(
            "optimization.mv_noise_std",
            &self.mv_noise_std,
            &vars.mvs,
            true,
        )?;
        self.metric_weights.validate()?;

        if model.and_then(ModelConfig::linear_state_space).is_none() {
            return Ok(());
        }
        if self.simulation_duration_s <= 0.0 || !self.simulation_duration_s.is_finite() {
            bail!("optimization.simulation_duration_s must be a finite value > 0");
        }
        validate_range_pair(
            "optimization.a_scale_pct",
            self.a_scale_min_pct,
            self.a_scale_max_pct,
        )?;
        validate_range_pair(
            "optimization.b_scale_pct",
            self.b_scale_min_pct,
            self.b_scale_max_pct,
        )?;
        if self.scenario_count == 0
            || self.max_iterations == 0
            || self.initial_samples == 0
            || self.convergence_patience == 0
            || self.plot_interval == 0
        {
            bail!("optimization counts and intervals must be > 0");
        }

        let active_keys: HashSet<_> = active_params.keys().map(|key| key.as_str()).collect();
        let mut seen = HashSet::new();
        for row in &self.parameters {
            if !active_keys.contains(row.key.as_str()) {
                bail!("optimization parameter '{}' is not active", row.key);
            }
            if !seen.insert(row.key.as_str()) {
                bail!("duplicate optimization parameter '{}'", row.key);
            }
            row.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterOptimizationParamConfig {
    pub key: String,
    #[serde(default)]
    pub fixed: bool,
    pub start: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineOptimizationConfig {
    #[serde(default)]
    pub parameters: Vec<OnlineOptimizationParamConfig>,
    #[serde(default)]
    pub cvs: Vec<OnlineOptimizationCvConfig>,
    #[serde(default = "default_online_optimization_duration_s")]
    pub duration_s: u64,
    #[serde(default = "default_optimization_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_optimization_initial_samples")]
    pub initial_samples: usize,
    #[serde(default = "default_optimization_convergence_patience")]
    pub convergence_patience: usize,
}

impl Default for OnlineOptimizationConfig {
    fn default() -> Self {
        Self {
            parameters: Vec::new(),
            cvs: Vec::new(),
            duration_s: default_online_optimization_duration_s(),
            max_iterations: default_optimization_max_iterations(),
            initial_samples: default_optimization_initial_samples(),
            convergence_patience: default_optimization_convergence_patience(),
        }
    }
}

impl OnlineOptimizationConfig {
    fn normalized_for(
        self,
        vars: &VariableCatalogConfig,
        active_params: &HashMap<String, f64>,
        simulation: &SimulationConfig,
    ) -> Self {
        let parameters = self
            .parameters
            .into_iter()
            .filter(|row| active_params.contains_key(&row.key))
            .map(|mut row| {
                if row.min > row.max {
                    std::mem::swap(&mut row.min, &mut row.max);
                }
                row
            })
            .collect();
        let mut existing_cvs: HashMap<_, _> = self
            .cvs
            .into_iter()
            .map(|row| (row.name.clone(), row))
            .collect();
        let cvs = vars
            .cvs
            .iter()
            .map(|name| {
                existing_cvs.remove(name).unwrap_or_else(|| {
                    let center = simulation
                        .cv_setpoints
                        .get(name)
                        .copied()
                        .or_else(|| simulation.cv_initials.get(name).copied())
                        .unwrap_or(0.0);
                    let width = center.abs().max(1.0) * 10.0;
                    OnlineOptimizationCvConfig {
                        name: name.clone(),
                        weight: 1.0,
                        low: center - width,
                        high: center + width,
                    }
                })
            })
            .collect();
        Self {
            parameters,
            cvs,
            duration_s: self.duration_s.clamp(1, 31_536_000),
            max_iterations: self.max_iterations.clamp(1, 10_000),
            initial_samples: self.initial_samples.clamp(1, 1_000),
            convergence_patience: self.convergence_patience.clamp(1, 1_000),
        }
    }

    fn validate(
        &self,
        vars: &VariableCatalogConfig,
        active_params: &HashMap<String, f64>,
    ) -> Result<()> {
        if self.duration_s == 0
            || self.max_iterations == 0
            || self.initial_samples == 0
            || self.convergence_patience == 0
        {
            bail!("online optimization duration and iteration settings must be > 0");
        }
        if self.parameters.is_empty() {
            return Ok(());
        }
        let mut seen = HashSet::new();
        for row in &self.parameters {
            if !active_params.contains_key(&row.key) {
                bail!("online optimization parameter '{}' is not active", row.key);
            }
            if !seen.insert(row.key.as_str()) {
                bail!("duplicate online optimization parameter '{}'", row.key);
            }
            if !row.min.is_finite() || !row.max.is_finite() || row.min >= row.max {
                bail!(
                    "online optimization parameter '{}' requires finite min < max",
                    row.key
                );
            }
        }
        let expected: HashSet<_> = vars.cvs.iter().map(String::as_str).collect();
        let mut cv_seen = HashSet::new();
        let mut positive_weight = false;
        for row in &self.cvs {
            if !expected.contains(row.name.as_str()) || !cv_seen.insert(row.name.as_str()) {
                bail!("invalid or duplicate online optimization CV '{}'", row.name);
            }
            if !row.weight.is_finite() || row.weight < 0.0 {
                bail!(
                    "online optimization CV '{}' weight must be finite and >= 0",
                    row.name
                );
            }
            positive_weight |= row.weight > 0.0;
            if !row.low.is_finite() || !row.high.is_finite() || row.low >= row.high {
                bail!(
                    "online optimization CV '{}' requires finite low < high",
                    row.name
                );
            }
        }
        if cv_seen.len() != expected.len() {
            bail!("online optimization requires safety limits for every CV");
        }
        if !vars.cvs.is_empty() && !positive_weight {
            bail!("online optimization requires at least one positive CV weight");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineOptimizationParamConfig {
    pub key: String,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineOptimizationCvConfig {
    pub name: String,
    pub weight: f64,
    pub low: f64,
    pub high: f64,
}

const fn default_online_optimization_duration_s() -> u64 {
    60
}

impl ParameterOptimizationParamConfig {
    fn from_default(key: String, default: f64) -> Self {
        let width = default.abs().max(1.0);
        Self {
            key,
            fixed: false,
            start: default,
            min: default - width,
            max: default + width,
        }
    }

    fn normalized_for_default(mut self, default: f64) -> Self {
        // Start is the currently saved controller value, not a separate optimization setting.
        self.start = default;
        if !self.min.is_finite() {
            self.min = self.start - self.start.abs().max(1.0);
        }
        if !self.max.is_finite() {
            self.max = self.start + self.start.abs().max(1.0);
        }
        if self.min > self.max {
            std::mem::swap(&mut self.min, &mut self.max);
        }
        self
    }

    fn validate(&self) -> Result<()> {
        if self.key.trim().is_empty() {
            bail!("optimization parameter key must not be empty");
        }
        if !self.start.is_finite() || !self.min.is_finite() || !self.max.is_finite() {
            bail!(
                "optimization parameter '{}' values must be finite",
                self.key
            );
        }
        if !self.fixed && self.min > self.max {
            bail!("optimization parameter '{}' has min > max", self.key);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ParameterOptimizationMetricWeights {
    #[serde(default = "default_metric_weight")]
    pub ise: f64,
    #[serde(default)]
    pub iae: f64,
    #[serde(default)]
    pub itae: f64,
    #[serde(default)]
    pub overshoot: f64,
    #[serde(default)]
    pub actuator: f64,
}

impl Default for ParameterOptimizationMetricWeights {
    fn default() -> Self {
        Self {
            ise: default_metric_weight(),
            iae: 0.0,
            itae: 0.0,
            overshoot: 0.0,
            actuator: 0.0,
        }
    }
}

impl ParameterOptimizationMetricWeights {
    fn normalized(self) -> Self {
        Self {
            ise: finite_or(self.ise, default_metric_weight()).max(0.0),
            iae: finite_or(self.iae, 0.0).max(0.0),
            itae: finite_or(self.itae, 0.0).max(0.0),
            overshoot: finite_or(self.overshoot, 0.0).max(0.0),
            actuator: finite_or(self.actuator, 0.0).max(0.0),
        }
    }

    fn validate(&self) -> Result<()> {
        if !self.ise.is_finite()
            || !self.iae.is_finite()
            || !self.itae.is_finite()
            || !self.overshoot.is_finite()
            || !self.actuator.is_finite()
        {
            bail!("optimization.metric_weights must be finite");
        }
        if self.ise < 0.0
            || self.iae < 0.0
            || self.itae < 0.0
            || self.overshoot < 0.0
            || self.actuator < 0.0
        {
            bail!("optimization.metric_weights must be >= 0");
        }
        if self.ise == 0.0
            && self.iae == 0.0
            && self.itae == 0.0
            && self.overshoot == 0.0
            && self.actuator == 0.0
        {
            bail!("at least one optimization metric weight must be > 0");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationConfig {
    #[serde(default)]
    pub speed: SimulationSpeedMode,
    #[serde(default)]
    pub cv_setpoints: HashMap<String, f64>,
    #[serde(default)]
    pub cv_initials: HashMap<String, f64>,
    #[serde(default)]
    pub mv_initials: HashMap<String, f64>,
    #[serde(default)]
    pub dv_initials: HashMap<String, f64>,
    #[serde(default)]
    pub cv_noise_std: HashMap<String, f64>,
    #[serde(default)]
    pub mv_noise_std: HashMap<String, f64>,
    #[serde(default)]
    pub dv_disturbances: HashMap<String, f64>,
    #[serde(default)]
    pub cv_offsets: HashMap<String, f64>,
    #[serde(default)]
    pub a_scale_pct: Vec<Vec<f64>>,
    #[serde(default)]
    pub b_scale_pct: Vec<Vec<f64>>,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            speed: SimulationSpeedMode::RealTime,
            cv_setpoints: HashMap::new(),
            cv_initials: HashMap::new(),
            mv_initials: HashMap::new(),
            dv_initials: HashMap::new(),
            cv_noise_std: HashMap::new(),
            mv_noise_std: HashMap::new(),
            dv_disturbances: HashMap::new(),
            cv_offsets: HashMap::new(),
            a_scale_pct: Vec::new(),
            b_scale_pct: Vec::new(),
        }
    }
}

impl SimulationConfig {
    fn normalized_for(self, vars: &VariableCatalogConfig, model: Option<&ModelConfig>) -> Self {
        let (a_rows, a_cols, b_rows, b_cols) = match model.and_then(ModelConfig::linear_state_space)
        {
            Some(model) => (
                model.state_count,
                model.state_count,
                model.state_count,
                model.u_order.len(),
            ),
            None => (0, 0, 0, 0),
        };

        Self {
            speed: self.speed,
            cv_setpoints: normalize_named_f64_map(self.cv_setpoints, &vars.cvs, 0.0),
            cv_initials: normalize_named_f64_map(self.cv_initials, &vars.cvs, 0.0),
            mv_initials: normalize_named_f64_map(self.mv_initials, &vars.mvs, 0.0),
            dv_initials: normalize_named_f64_map(self.dv_initials, &vars.dvs, 0.0),
            cv_noise_std: normalize_named_f64_map(self.cv_noise_std, &vars.cvs, 0.0),
            mv_noise_std: normalize_named_f64_map(self.mv_noise_std, &vars.mvs, 0.0),
            dv_disturbances: normalize_named_f64_map(self.dv_disturbances, &vars.dvs, 0.0),
            cv_offsets: normalize_named_f64_map(self.cv_offsets, &vars.cvs, 0.0),
            a_scale_pct: resize_matrix_fill(self.a_scale_pct, a_rows, a_cols, 100.0),
            b_scale_pct: resize_matrix_fill(self.b_scale_pct, b_rows, b_cols, 100.0),
        }
    }

    fn validate(&self, model: Option<&ModelConfig>, vars: &VariableCatalogConfig) -> Result<()> {
        validate_named_f64_values(
            "simulation.cv_setpoints",
            &self.cv_setpoints,
            &vars.cvs,
            false,
        )?;
        validate_named_f64_values(
            "simulation.cv_initials",
            &self.cv_initials,
            &vars.cvs,
            false,
        )?;
        validate_named_f64_values(
            "simulation.mv_initials",
            &self.mv_initials,
            &vars.mvs,
            false,
        )?;
        validate_named_f64_values(
            "simulation.dv_initials",
            &self.dv_initials,
            &vars.dvs,
            false,
        )?;
        validate_named_f64_values(
            "simulation.cv_noise_std",
            &self.cv_noise_std,
            &vars.cvs,
            true,
        )?;
        validate_named_f64_values(
            "simulation.mv_noise_std",
            &self.mv_noise_std,
            &vars.mvs,
            true,
        )?;
        validate_named_f64_values(
            "simulation.dv_disturbances",
            &self.dv_disturbances,
            &vars.dvs,
            false,
        )?;
        validate_named_f64_values("simulation.cv_offsets", &self.cv_offsets, &vars.cvs, false)?;

        let Some(model) = model.and_then(ModelConfig::linear_state_space) else {
            if !self.a_scale_pct.is_empty() || !self.b_scale_pct.is_empty() {
                bail!("simulation.a_scale_pct and simulation.b_scale_pct require a linear model");
            }
            return Ok(());
        };

        validate_matrix_finite(
            "simulation.a_scale_pct",
            &self.a_scale_pct,
            model.state_count,
            model.state_count,
        )?;
        validate_matrix_finite(
            "simulation.b_scale_pct",
            &self.b_scale_pct,
            model.state_count,
            model.u_order.len(),
        )?;
        Ok(())
    }
}

fn linear_model_u_order(vars: &VariableCatalogConfig) -> Vec<String> {
    vars.mvs
        .iter()
        .cloned()
        .chain(vars.dvs.iter().cloned())
        .collect()
}

fn linear_model_y_order(vars: &VariableCatalogConfig) -> Vec<String> {
    vars.cvs.clone()
}

fn validate_matrix(name: &str, matrix: &[Vec<f64>], rows: usize, cols: usize) -> Result<()> {
    if matrix.len() != rows {
        bail!(
            "model.linear_state_space.{name_lower} must have shape {}x{}",
            rows,
            cols,
            name_lower = name.to_ascii_lowercase()
        );
    }

    for (row_index, row) in matrix.iter().enumerate() {
        if row.len() != cols {
            bail!(
                "model.linear_state_space.{name_lower}[{}] must have {} columns",
                row_index,
                cols,
                name_lower = name.to_ascii_lowercase()
            );
        }

        for value in row {
            if !value.is_finite() {
                bail!(
                    "model.linear_state_space.{name_lower} must contain only finite numbers",
                    name_lower = name.to_ascii_lowercase()
                );
            }
        }
    }

    Ok(())
}

fn validate_matrix_finite(name: &str, matrix: &[Vec<f64>], rows: usize, cols: usize) -> Result<()> {
    if matrix.len() != rows {
        bail!("{name} must have shape {}x{}", rows, cols);
    }

    for (row_index, row) in matrix.iter().enumerate() {
        if row.len() != cols {
            bail!("{name}[{}] must have {} columns", row_index, cols);
        }

        for value in row {
            if !value.is_finite() {
                bail!("{name} must contain only finite numbers");
            }
        }
    }

    Ok(())
}

fn resize_matrix_overlap(matrix: Vec<Vec<f64>>, rows: usize, cols: usize) -> Vec<Vec<f64>> {
    resize_matrix_fill(matrix, rows, cols, 0.0)
}

fn resize_matrix_fill(matrix: Vec<Vec<f64>>, rows: usize, cols: usize, fill: f64) -> Vec<Vec<f64>> {
    let mut resized = vec![vec![fill; cols]; rows];

    for (row_index, row) in matrix.into_iter().enumerate().take(rows) {
        for (col_index, value) in row.into_iter().enumerate().take(cols) {
            resized[row_index][col_index] = value;
        }
    }

    resized
}

fn normalize_named_f64_map(
    values: HashMap<String, f64>,
    names: &[String],
    default: f64,
) -> HashMap<String, f64> {
    let mut normalized = HashMap::new();
    for name in names {
        normalized.insert(name.clone(), values.get(name).copied().unwrap_or(default));
    }
    normalized
}

fn normalize_named_f64_map_with<F>(
    values: HashMap<String, f64>,
    names: &[String],
    mut default_for: F,
) -> HashMap<String, f64>
where
    F: FnMut(&str) -> f64,
{
    let mut normalized = HashMap::new();
    for name in names {
        normalized.insert(
            name.clone(),
            values
                .get(name)
                .copied()
                .filter(|value| value.is_finite())
                .unwrap_or_else(|| default_for(name)),
        );
    }
    normalized
}

fn validate_named_f64_values(
    field: &str,
    values: &HashMap<String, f64>,
    allowed_names: &[String],
    require_non_negative: bool,
) -> Result<()> {
    let allowed: HashSet<_> = allowed_names.iter().map(|name| name.as_str()).collect();
    for (name, value) in values {
        if !allowed.contains(name.as_str()) {
            bail!("{field} references unknown variable '{}'", name);
        }
        if !value.is_finite() {
            bail!("{field} must contain only finite numbers");
        }
        if require_non_negative && *value < 0.0 {
            bail!("{field} must be >= 0");
        }
    }
    Ok(())
}

fn validate_range_pair(name: &str, min: f64, max: f64) -> Result<()> {
    if !min.is_finite() || !max.is_finite() {
        bail!("{name} range values must be finite");
    }
    if min > max {
        bail!("{name} min must be <= max");
    }
    Ok(())
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

fn default_a_scale_min_pct() -> f64 {
    90.0
}

fn default_a_scale_max_pct() -> f64 {
    110.0
}

fn default_b_scale_min_pct() -> f64 {
    90.0
}

fn default_b_scale_max_pct() -> f64 {
    110.0
}

fn default_optimization_scenario_count() -> usize {
    8
}

fn default_optimization_max_iterations() -> usize {
    30
}

fn default_optimization_initial_samples() -> usize {
    8
}

fn default_optimization_convergence_patience() -> usize {
    10
}

fn default_optimization_plot_interval() -> usize {
    5
}

fn default_optimization_duration_s() -> f64 {
    120.0
}

fn default_metric_weight() -> f64 {
    1.0
}

fn remap_matrix_rows(
    matrix: Vec<Vec<f64>>,
    old_rows: &[String],
    new_rows: &[String],
    cols: usize,
) -> Vec<Vec<f64>> {
    let resized = resize_matrix_overlap(matrix, old_rows.len(), cols);
    let row_lookup: HashMap<&str, usize> = old_rows
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let mut out = vec![vec![0.0; cols]; new_rows.len()];

    for (new_row_index, name) in new_rows.iter().enumerate() {
        if let Some(&old_row_index) = row_lookup.get(name.as_str()) {
            out[new_row_index].clone_from(&resized[old_row_index]);
        }
    }

    out
}

fn remap_matrix_columns(
    matrix: Vec<Vec<f64>>,
    rows: usize,
    old_cols: &[String],
    new_cols: &[String],
) -> Vec<Vec<f64>> {
    let resized = resize_matrix_overlap(matrix, rows, old_cols.len());
    let col_lookup: HashMap<&str, usize> = old_cols
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let mut out = vec![vec![0.0; new_cols.len()]; rows];

    for row_index in 0..rows {
        for (new_col_index, name) in new_cols.iter().enumerate() {
            if let Some(&old_col_index) = col_lookup.get(name.as_str()) {
                out[row_index][new_col_index] = resized[row_index][old_col_index];
            }
        }
    }

    out
}

fn remap_matrix_rows_and_columns(
    matrix: Vec<Vec<f64>>,
    old_rows: &[String],
    new_rows: &[String],
    old_cols: &[String],
    new_cols: &[String],
) -> Vec<Vec<f64>> {
    let resized = resize_matrix_overlap(matrix, old_rows.len(), old_cols.len());
    let row_lookup: HashMap<&str, usize> = old_rows
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let col_lookup: HashMap<&str, usize> = old_cols
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let mut out = vec![vec![0.0; new_cols.len()]; new_rows.len()];

    for (new_row_index, row_name) in new_rows.iter().enumerate() {
        let Some(&old_row_index) = row_lookup.get(row_name.as_str()) else {
            continue;
        };

        for (new_col_index, col_name) in new_cols.iter().enumerate() {
            if let Some(&old_col_index) = col_lookup.get(col_name.as_str()) {
                out[new_row_index][new_col_index] = resized[old_row_index][old_col_index];
            }
        }
    }

    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub execution_time_test: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            execution_time_test: true,
        }
    }
}

const fn default_cycle_ms() -> u64 {
    1_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlarmConfig {
    #[serde(default = "default_alarm_timeout_ms")]
    pub alarm_timeout_ms: u64,
    #[serde(default = "default_off_timeout_ms")]
    pub off_timeout_ms: u64,
}

impl Default for AlarmConfig {
    fn default() -> Self {
        Self {
            alarm_timeout_ms: default_alarm_timeout_ms(),
            off_timeout_ms: default_off_timeout_ms(),
        }
    }
}

const fn default_alarm_timeout_ms() -> u64 {
    5_000
}

const fn default_off_timeout_ms() -> u64 {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentConfig {
    pub service_name: String,
    pub display_name: String,
    pub description: String,
}

impl DeploymentConfig {
    fn default_for_project(project: &str) -> Self {
        Self {
            service_name: format!("smartapc_{}", project),
            display_name: format!("SmartAPC {}", project),
            description: "SmartAPC controller runtime".to_string(),
        }
    }
}

pub fn from_toml_str(input: &str) -> Result<ProjectConfig> {
    let mut value: toml::Value = toml::from_str(input).context("failed to parse project TOML")?;
    migrate_legacy_timing(&mut value);
    let mut cfg: ProjectConfig = value.try_into().context("failed to decode project TOML")?;
    cfg.normalize_model_config();
    cfg.normalize_simulation_config();
    cfg.normalize_optimization_config();
    cfg.normalize_online_optimization_config();
    cfg.validate()?;
    Ok(cfg)
}

fn migrate_legacy_timing(value: &mut toml::Value) {
    let Some(root) = value.as_table_mut() else {
        return;
    };

    let legacy_io_cycle = root
        .get_mut("communication")
        .and_then(toml::Value::as_table_mut)
        .and_then(|communication| {
            communication
                .remove("io_cycle_ms")
                .and_then(|value| value.as_integer())
        });

    if let Some(cycle_ms) = legacy_io_cycle {
        let communication = root
            .entry("communication")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .expect("communication table");
        communication
            .entry("cycle_ms")
            .or_insert(toml::Value::Integer(cycle_ms));

        let io = root
            .entry("io")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .expect("io table");
        io.entry("cycle_ms")
            .or_insert(toml::Value::Integer(cycle_ms));
    }

    let legacy_controller_cycle = root
        .get_mut("runtime")
        .and_then(toml::Value::as_table_mut)
        .and_then(|runtime| {
            runtime
                .remove("cycle_ms")
                .and_then(|value| value.as_integer())
        });
    if let Some(cycle_ms) = legacy_controller_cycle {
        let logic = root
            .entry("logic")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .expect("logic table");
        logic
            .entry("cycle_ms")
            .or_insert(toml::Value::Integer(cycle_ms));
    }

    if let Some(watchdog) = root.get_mut("watchdog").and_then(toml::Value::as_table_mut) {
        watchdog.remove("mirror_cycle_ms");
    }
}

pub fn to_toml_string(cfg: &ProjectConfig) -> Result<String> {
    let mut normalized = cfg.clone();
    normalized.normalize_plot_config();
    normalized.normalize_model_config();
    normalized.normalize_simulation_config();
    normalized.normalize_optimization_config();
    normalized.validate()?;
    toml::to_string_pretty(&normalized).context("failed to encode project TOML")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config_with_variables() -> ProjectConfig {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string(), "u2".to_string()];
        cfg.variables.cvs = vec!["y1".to_string(), "y2".to_string()];
        cfg.variables.dvs = vec!["d1".to_string()];
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.setpoint".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_SETPOINT".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=U1_WRITE".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.process_value".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_PROCESS_VALUE".to_string(),
            opc_tag: None,
        });
        cfg.logic.p_controller.pairs.push(PControllerPairConfig {
            cv: "y1".to_string(),
            mv: "u1".to_string(),
            kp: 1.0,
            bias: 0.0,
        });

        cfg
    }

    fn sample_linear_model() -> ModelConfig {
        ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 3,
                u_order: vec!["u1".to_string(), "u2".to_string(), "d1".to_string()],
                y_order: vec!["y1".to_string(), "y2".to_string()],
                a: vec![
                    vec![0.0, 1.0, 0.0],
                    vec![0.0, 0.0, 1.0],
                    vec![-1.0, -2.0, -3.0],
                ],
                b: vec![
                    vec![0.0, 0.0, 0.0],
                    vec![1.0, 0.0, 0.0],
                    vec![0.0, 1.0, 0.5],
                ],
                c: vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]],
                d: vec![vec![0.0, 0.0, 0.0], vec![0.0, 0.0, 0.0]],
            },
        }
    }

    #[test]
    fn default_config_is_valid() {
        let cfg = valid_config_with_variables();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn invalid_cycle_rejected() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.logic.cycle_ms = 0;
        let err = cfg.validate().expect_err("expected cycle validation error");
        assert!(err.to_string().contains("cycle_ms"));
    }

    #[test]
    fn io_cycle_must_align_with_communication_cycle() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.communication.cycle_ms = 300;
        cfg.io.cycle_ms = 1_000;
        let err = cfg.validate().expect_err("expected cycle alignment error");
        assert!(err.to_string().contains("multiple"));
    }

    #[test]
    fn toml_roundtrip() {
        let mut cfg = valid_config_with_variables();
        cfg.io.inputs[0].opc_tag = Some("TAG_IN".to_string());
        cfg.logic.p_controller.pairs[0].kp = 2.0;
        cfg.logic.p_controller.pairs[0].bias = 0.5;
        cfg.logic
            .params
            .insert("gain_override".to_string(), LogicParamValue::F64(2.5));

        let encoded = to_toml_string(&cfg).expect("toml encode");
        let decoded = from_toml_str(&encoded).expect("toml decode");
        assert_eq!(decoded.name, "demo");
    }

    #[test]
    fn linear_model_roundtrip_uses_typed_shape() {
        let mut cfg = valid_config_with_variables();
        cfg.model = Some(sample_linear_model());

        let encoded = to_toml_string(&cfg).expect("toml encode");
        let decoded = from_toml_str(&encoded).expect("toml decode");

        assert!(encoded.contains("kind = \"linear_state_space\""));
        assert_eq!(decoded.model, Some(sample_linear_model()));
    }

    #[test]
    fn optimization_config_roundtrip_and_normalization() {
        let mut cfg = valid_config_with_variables();
        cfg.model = Some(sample_linear_model());
        cfg.sync_active_online_params();
        cfg.normalize_optimization_config();
        cfg.optimization.metric_weights.ise = 0.0;
        cfg.optimization.metric_weights.iae = 1.0;
        cfg.optimization.metric_weights.overshoot = 2.0;
        cfg.optimization.metric_weights.actuator = 0.5;
        cfg.optimization.scenario_count = 3;
        let active_params = cfg.active_controller_f64_param_defaults();
        let expected_start = active_params["p.y1.u1.kp"];
        cfg.optimization
            .parameters
            .iter_mut()
            .find(|row| row.key == "p.y1.u1.kp")
            .expect("optimization parameter")
            .start = expected_start + 123.0;

        let encoded = to_toml_string(&cfg).expect("toml encode");
        let decoded = from_toml_str(&encoded).expect("toml decode");

        assert_eq!(decoded.optimization.scenario_count, 3);
        assert_eq!(decoded.optimization.metric_weights.iae, 1.0);
        assert_eq!(decoded.optimization.metric_weights.overshoot, 2.0);
        assert_eq!(decoded.optimization.metric_weights.actuator, 0.5);
        let kp_row = decoded
            .optimization
            .parameters
            .iter()
            .find(|row| row.key == "p.y1.u1.kp")
            .expect("optimization parameter");
        assert_eq!(kp_row.start, expected_start);
    }

    #[test]
    fn sidebar_places_optimization_after_kalman_tuning() {
        let cfg = valid_config_with_variables();
        let tasks = cfg.sidebar_tasks();
        let tuning_index = tasks
            .iter()
            .position(|task| task.id == "kalman_tuning")
            .expect("kalman tuning task");
        let optimization_index = tasks
            .iter()
            .position(|task| task.id == "optimization")
            .expect("optimization task");
        assert_eq!(optimization_index, tuning_index + 1);
    }

    #[test]
    fn online_optimization_normalizes_cvs_and_selected_numeric_parameters() {
        let mut cfg = valid_config_with_variables();
        cfg.sync_active_online_params();
        let active_key = cfg
            .active_controller_f64_param_defaults()
            .into_keys()
            .next()
            .expect("active numeric parameter");
        cfg.online_optimization.parameters = vec![
            OnlineOptimizationParamConfig {
                key: active_key,
                min: 2.0,
                max: -2.0,
            },
            OnlineOptimizationParamConfig {
                key: "not.active".to_string(),
                min: 0.0,
                max: 1.0,
            },
        ];
        cfg.normalize_online_optimization_config();

        assert_eq!(cfg.online_optimization.parameters.len(), 1);
        assert_eq!(cfg.online_optimization.parameters[0].min, -2.0);
        assert_eq!(cfg.online_optimization.cvs.len(), cfg.variables.cvs.len());
        cfg.validate()
            .expect("normalized online optimization config");
    }

    #[test]
    fn online_optimization_requires_positive_cv_weight_when_configured() {
        let mut cfg = valid_config_with_variables();
        cfg.sync_active_online_params();
        let active_key = cfg
            .active_controller_f64_param_defaults()
            .into_keys()
            .next()
            .expect("active numeric parameter");
        cfg.online_optimization.parameters = vec![OnlineOptimizationParamConfig {
            key: active_key,
            min: 0.0,
            max: 2.0,
        }];
        cfg.normalize_online_optimization_config();
        for cv in &mut cfg.online_optimization.cvs {
            cv.weight = 0.0;
        }

        let err = cfg.validate().expect_err("zero weights must fail");
        assert!(err.to_string().contains("positive CV weight"));
    }

    #[test]
    fn online_optimization_config_roundtrips() {
        let mut cfg = valid_config_with_variables();
        cfg.sync_active_online_params();
        let active_key = cfg
            .active_f64_online_param_defaults()
            .into_keys()
            .next()
            .expect("active numeric parameter");
        cfg.online_optimization.parameters = vec![OnlineOptimizationParamConfig {
            key: active_key,
            min: -1.0,
            max: 3.0,
        }];
        cfg.online_optimization.duration_s = 3_723;
        cfg.normalize_online_optimization_config();

        let encoded = to_toml_string(&cfg).expect("encode");
        let decoded = from_toml_str(&encoded).expect("decode");
        assert_eq!(decoded.online_optimization.duration_s, 3_723);
        assert_eq!(decoded.online_optimization.parameters.len(), 1);
        assert_eq!(
            decoded.online_optimization.cvs.len(),
            cfg.variables.cvs.len()
        );
    }

    #[test]
    fn invalid_linear_model_shape_is_rejected() {
        let mut cfg = valid_config_with_variables();
        cfg.model = Some(ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 2,
                u_order: vec!["u1".to_string(), "u2".to_string(), "d1".to_string()],
                y_order: vec!["y1".to_string(), "y2".to_string()],
                a: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
                b: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
                c: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
                d: vec![vec![0.0, 0.0, 0.0], vec![0.0, 0.0, 0.0]],
            },
        });

        let err = cfg.validate().expect_err("expected invalid matrix shape");
        assert!(err.to_string().contains("model.linear_state_space.b"));
    }

    #[test]
    fn legacy_noop_model_is_accepted_and_normalized_to_none() {
        let input = r#"
name = "demo"

[communication]
protocol = "opc_ua"
endpoint_url = "opc.tcp://127.0.0.1:4840"
io_cycle_ms = 1000

[watchdog]
kind = "none"

[variables]
mvs = ["u1", "u2"]
cvs = ["y1", "y2"]
dvs = ["d1"]

[io]
inputs = [
  { name = "cv.y1.setpoint", signal_type = "f64", address = "ns=2;s=Y1_SETPOINT" },
  { name = "cv.y1.process_value", signal_type = "f64", address = "ns=2;s=Y1_PROCESS_VALUE" }
]
outputs = [
  { name = "mv.u1.write", signal_type = "f64", address = "ns=2;s=U1_WRITE" }
]

[limits]

[logic]
kind = "p_controller"

[logic.p_controller]
pairs = [{ cv = "y1", mv = "u1", kp = 1.0, bias = 0.0 }]

[model]
kind = "noop"

[model.config]
note = "legacy placeholder"

[runtime]
cycle_ms = 1000
execution_time_test = true

[alarms]
alarm_timeout_ms = 5000
off_timeout_ms = 10000

[deployment]
service_name = "smartapc_demo"
display_name = "SmartAPC demo"
description = "SmartAPC controller runtime"
"#;

        let decoded = from_toml_str(input).expect("legacy noop model should parse");
        assert!(decoded.model.is_none());
        assert_eq!(decoded.communication.cycle_ms, 1_000);
        assert_eq!(decoded.io.cycle_ms, 1_000);
        assert_eq!(decoded.logic.cycle_ms, 1_000);

        let encoded = to_toml_string(&decoded).expect("toml encode");
        assert!(!encoded.contains("[model]"));
        assert!(!encoded.contains("io_cycle_ms"));
        assert!(!encoded.contains("mirror_cycle_ms"));
    }

    #[test]
    fn linear_model_normalization_preserves_overlap_for_variable_changes() {
        let mut cfg = valid_config_with_variables();
        cfg.model = Some(sample_linear_model());

        cfg.variables.mvs = vec!["u2".to_string(), "u3".to_string()];
        cfg.variables.cvs = vec!["y2".to_string(), "y3".to_string()];
        cfg.variables.dvs = vec!["d1".to_string(), "d2".to_string()];
        cfg.normalize_model_config();

        let Some(ModelConfig::LinearStateSpace { model }) = cfg.model else {
            panic!("expected normalized linear state-space model");
        };

        assert_eq!(model.u_order, vec!["u2", "u3", "d1", "d2"]);
        assert_eq!(model.y_order, vec!["y2", "y3"]);
        assert_eq!(model.b[2], vec![1.0, 0.0, 0.5, 0.0]);
        assert_eq!(model.c[0], vec![0.0, 1.0, 0.0]);
        assert_eq!(model.c[1], vec![0.0, 0.0, 0.0]);
        assert_eq!(model.d[0], vec![0.0, 0.0, 0.0, 0.0]);
        assert_eq!(model.d[1], vec![0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn optional_model_and_simulation_do_not_block_sidebar_progress() {
        let cfg = valid_config_with_variables();

        assert!(cfg.model.is_none());
        assert!(cfg.validate().is_ok());

        let tasks = cfg.sidebar_tasks();
        let model_task = tasks
            .iter()
            .find(|task| task.id == "model")
            .expect("model task present");
        let simulation_task = tasks
            .iter()
            .find(|task| task.id == "simulation")
            .expect("simulation task present");

        assert_eq!(model_task.label, "Define Model (optional)");
        assert!(model_task.configured);
        assert_eq!(simulation_task.label, "Simulate Controller (optional)");
        assert!(simulation_task.configured);
    }

    #[test]
    fn simulation_normalization_tracks_names_and_resizes_scale_matrices() {
        let mut cfg = valid_config_with_variables();
        cfg.model = Some(sample_linear_model());
        cfg.simulation.cv_setpoints.insert("y2".to_string(), 2.5);
        cfg.simulation
            .cv_setpoints
            .insert("unused".to_string(), 99.0);
        cfg.simulation.a_scale_pct = vec![vec![50.0]];
        cfg.simulation.b_scale_pct = vec![vec![75.0]];

        cfg.normalize_simulation_config();

        assert_eq!(cfg.simulation.cv_setpoints.get("y1"), Some(&0.0));
        assert_eq!(cfg.simulation.cv_setpoints.get("y2"), Some(&2.5));
        assert!(!cfg.simulation.cv_setpoints.contains_key("unused"));
        assert_eq!(cfg.simulation.a_scale_pct.len(), 3);
        assert_eq!(cfg.simulation.a_scale_pct[0].len(), 3);
        assert_eq!(cfg.simulation.a_scale_pct[0][0], 50.0);
        assert_eq!(cfg.simulation.a_scale_pct[2][2], 100.0);
        assert_eq!(cfg.simulation.b_scale_pct.len(), 3);
        assert_eq!(cfg.simulation.b_scale_pct[0].len(), 3);
        assert_eq!(cfg.simulation.b_scale_pct[0][0], 75.0);
        assert_eq!(cfg.simulation.b_scale_pct[2][2], 100.0);

        cfg.variables.mvs.push("u3".to_string());
        cfg.variables.cvs.push("y3".to_string());
        cfg.variables.dvs.push("d2".to_string());
        cfg.normalize_model_config();
        cfg.normalize_simulation_config();

        assert_eq!(cfg.simulation.cv_setpoints.get("y3"), Some(&0.0));
        assert_eq!(cfg.simulation.a_scale_pct.len(), 3);
        assert_eq!(cfg.simulation.a_scale_pct[0].len(), 3);
        assert_eq!(cfg.simulation.b_scale_pct.len(), 3);
        assert_eq!(cfg.simulation.b_scale_pct[0].len(), 5);
        assert_eq!(cfg.simulation.b_scale_pct[0][0], 75.0);
        assert_eq!(cfg.simulation.b_scale_pct[2][4], 100.0);
    }

    #[test]
    fn simulation_validation_uses_synthetic_io_without_runtime_opc_addresses() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: String::new(),
            mv_read: String::new(),
            hl_opc: None,
            ll_opc: None,
            hl: Some(10.0),
            ll: Some(-10.0),
            enable: None,
        });
        cfg.io_definition.cvs.push(CvIoDefinition {
            name: "y1".to_string(),
            setpoint: String::new(),
            process_value: String::new(),
            deadband_opc: None,
            deadband_value: Some(0.0),
            enable: None,
        });
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller.pairs.push(PControllerPairConfig {
            cv: "y1".to_string(),
            mv: "u1".to_string(),
            kp: 1.0,
            bias: 0.0,
        });
        cfg.model = Some(ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 1,
                u_order: vec!["u1".to_string()],
                y_order: vec!["y1".to_string()],
                a: vec![vec![-1.0]],
                b: vec![vec![1.0]],
                c: vec![vec![1.0]],
                d: vec![vec![0.0]],
            },
        });
        cfg.normalize_simulation_config();

        let (sim_io, sim_limits) = cfg.build_simulation_io_and_limits().expect("simulation io");
        let input_names: Vec<_> = sim_io
            .inputs
            .iter()
            .map(|signal| signal.name.as_str())
            .collect();
        let output_names: Vec<_> = sim_io
            .outputs
            .iter()
            .map(|signal| signal.name.as_str())
            .collect();
        assert_eq!(input_names, vec!["cv.y1.setpoint", "cv.y1.process_value"]);
        assert_eq!(output_names, vec!["mv.u1.write"]);
        assert!(sim_limits.values.contains_key("mv.u1.write"));
        assert!(cfg.validate_for_simulation().is_ok());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn simulation_reuses_runtime_io_contract_for_compatible_custom_logic() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: "ns=2;s=U1_WRITE".to_string(),
            mv_read: "ns=2;s=U1_READ".to_string(),
            hl_opc: None,
            ll_opc: None,
            hl: Some(100.0),
            ll: Some(-100.0),
            enable: None,
        });
        cfg.io_definition.cvs.push(CvIoDefinition {
            name: "y1".to_string(),
            setpoint: "ns=2;s=Y1_SETPOINT".to_string(),
            process_value: "ns=2;s=Y1_PROCESS_VALUE".to_string(),
            deadband_opc: None,
            deadband_value: Some(0.0),
            enable: None,
        });
        cfg.rebuild_runtime_io_from_definition()
            .expect("rebuild runtime io");
        cfg.logic.kind = LogicKind::CustomRust;
        cfg.logic.custom_rust.source_path = "logic/custom_strategy/src/lib.rs".to_string();
        cfg.logic.custom_rust.artifact_path =
            "logic/custom_strategy/target/release/demo.dll".to_string();
        cfg.logic.custom_rust.required_inputs = vec![
            "mv.u1.read".to_string(),
            "cv.y1.setpoint".to_string(),
            "cv.y1.process_value".to_string(),
        ];
        cfg.logic.custom_rust.required_outputs = vec!["mv.u1.write".to_string()];
        cfg.logic.custom_rust.compatible = true;
        cfg.model = Some(ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 1,
                u_order: vec!["u1".to_string()],
                y_order: vec!["y1".to_string()],
                a: vec![vec![-1.0]],
                b: vec![vec![1.0]],
                c: vec![vec![1.0]],
                d: vec![vec![0.0]],
            },
        });
        cfg.normalize_simulation_config();

        let (sim_io, _) = cfg.build_simulation_io_and_limits().expect("simulation io");
        let input_names: Vec<_> = sim_io
            .inputs
            .iter()
            .map(|signal| signal.name.as_str())
            .collect();
        let output_names: Vec<_> = sim_io
            .outputs
            .iter()
            .map(|signal| signal.name.as_str())
            .collect();

        assert_eq!(
            input_names,
            vec!["mv.u1.read", "cv.y1.setpoint", "cv.y1.process_value"]
        );
        assert_eq!(output_names, vec!["mv.u1.write"]);
        assert!(cfg.validate_for_simulation().is_ok());
    }

    #[test]
    fn sidebar_uses_new_io_tasks_and_no_params_step() {
        let cfg = ProjectConfig::new("demo");
        let tasks = cfg.sidebar_tasks();

        assert!(
            tasks
                .iter()
                .any(|task| { task.id == "io" && task.label == "Define Inputs and Outputs" })
        );
        assert!(
            tasks
                .iter()
                .any(|task| task.id == "limits" && task.label == "Define I/O")
        );
        assert!(!tasks.iter().any(|task| task.id == "params"));
    }

    #[test]
    fn sidebar_places_kalman_tuning_after_logic() {
        let cfg = ProjectConfig::new("demo");
        let tasks = cfg.sidebar_tasks();
        let logic = tasks
            .iter()
            .position(|task| task.id == "logic")
            .expect("logic");
        let tuning = tasks
            .iter()
            .position(|task| task.id == "kalman_tuning")
            .expect("kalman tuning");
        let optimization = tasks
            .iter()
            .position(|task| task.id == "optimization")
            .expect("optimization");
        assert_eq!(tuning, logic + 1);
        assert_eq!(optimization, tuning + 1);
        assert!(!tasks[tuning].configured);
    }

    #[test]
    fn custom_logic_compatibility_requires_exact_io_match() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.setpoint".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_SETPOINT".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.process_value".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_PROCESS_VALUE".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=U1_WRITE".to_string(),
            opc_tag: None,
        });

        cfg.logic.kind = LogicKind::CustomRust;
        cfg.logic.custom_rust.source_path = "logic/custom_strategy/src/lib.rs".to_string();
        cfg.logic.custom_rust.artifact_path =
            "logic/custom_strategy/target/release/demo.dll".to_string();
        cfg.logic.custom_rust.required_inputs = vec!["cv.y1.setpoint".to_string()];
        cfg.logic.custom_rust.required_outputs = vec!["mv.u1.write".to_string()];
        cfg.logic.custom_rust.compatible = true;

        assert!(!cfg.logic.custom_rust.is_compatible_with(&cfg.io));

        cfg.logic.custom_rust.required_inputs = vec![
            "cv.y1.setpoint".to_string(),
            "cv.y1.process_value".to_string(),
        ];
        assert!(cfg.logic.custom_rust.is_compatible_with(&cfg.io));
    }

    #[test]
    fn sync_active_online_params_prunes_unused_logic_params() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller.pairs = vec![PControllerPairConfig {
            cv: "y1".to_string(),
            mv: "u1".to_string(),
            kp: 2.0,
            bias: 1.0,
        }];
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: String::new(),
            mv_read: String::new(),
            hl_opc: None,
            ll_opc: None,
            hl: Some(10.0),
            ll: None,
            enable: None,
        });

        cfg.logic
            .params
            .insert("p.y1.u1.kp".to_string(), LogicParamValue::F64(3.0));
        cfg.logic
            .params
            .insert("custom_gain".to_string(), LogicParamValue::F64(5.0));
        cfg.logic
            .params
            .insert("io.mv.u1.hl".to_string(), LogicParamValue::F64(12.0));

        cfg.sync_active_online_params();

        assert!(cfg.logic.params.contains_key("p.y1.u1.kp"));
        assert!(cfg.logic.params.contains_key("p.y1.u1.bias"));
        assert!(cfg.logic.params.contains_key("io.mv.u1.hl"));
        assert!(!cfg.logic.params.contains_key("custom_gain"));
    }

    #[test]
    fn linear_mpc_requires_linear_model() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.setpoint".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_SETPOINT".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.process_value".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_PROCESS_VALUE".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=U1_WRITE".to_string(),
            opc_tag: None,
        });
        cfg.logic.kind = LogicKind::LinearMpc;

        let err = cfg.validate().expect_err("linear_mpc requires model");
        assert!(err.to_string().contains("linear state-space model"));
    }

    #[test]
    fn linear_mpc_syncs_active_online_params() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: "ns=2;s=U1_WRITE".to_string(),
            mv_read: String::new(),
            hl_opc: None,
            ll_opc: None,
            hl: Some(10.0),
            ll: Some(-10.0),
            enable: None,
        });
        cfg.io_definition.cvs.push(CvIoDefinition {
            name: "y1".to_string(),
            setpoint: "ns=2;s=Y1_SETPOINT".to_string(),
            process_value: "ns=2;s=Y1_PROCESS_VALUE".to_string(),
            deadband_opc: None,
            deadband_value: Some(2.0),
            enable: None,
        });
        cfg.rebuild_runtime_io_from_definition()
            .expect("runtime io");
        cfg.model = Some(ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 1,
                u_order: vec!["u1".to_string()],
                y_order: vec!["y1".to_string()],
                a: vec![vec![-1.0]],
                b: vec![vec![1.0]],
                c: vec![vec![1.0]],
                d: vec![vec![0.0]],
            },
        });
        cfg.logic.kind = LogicKind::LinearMpc;
        cfg.logic
            .params
            .insert("custom_gain".to_string(), LogicParamValue::F64(5.0));
        cfg.logic
            .params
            .insert("mpc.cv.y1.q".to_string(), LogicParamValue::F64(3.0));

        cfg.sync_active_online_params();

        assert_eq!(
            cfg.logic.params.get("mpc.cv.y1.q"),
            Some(&LogicParamValue::F64(3.0))
        );
        assert_eq!(
            cfg.logic.params.get("mpc.cv.y1.band_low"),
            Some(&LogicParamValue::F64(-2.0))
        );
        assert!(cfg.logic.params.contains_key("mpc.mv.u1.r"));
        assert!(cfg.logic.params.contains_key("mpc.kalman.state.1.q"));
        assert!(cfg.logic.params.contains_key("io.mv.u1.hl"));
        assert!(!cfg.logic.params.contains_key("custom_gain"));
        assert!(cfg.linear_mpc_is_configured_for(&cfg.io));

        cfg.normalize_optimization_config();
        let q_row = cfg
            .optimization
            .parameters
            .iter()
            .find(|row| row.key == "mpc.cv.y1.q")
            .expect("optimization q parameter");
        assert_eq!(q_row.start, 3.0);
    }

    #[test]
    fn normalize_plot_config_clamps_window_and_orders() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string(), "u2".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.variables.dvs = vec!["d1".to_string(), "d2".to_string()];

        cfg.plots.window_sec = 8;
        cfg.plots.future_window_sec = 4_000;
        cfg.plots.mv_order = vec![
            "u2".to_string(),
            "u2".to_string(),
            "unknown".to_string(),
            "u1".to_string(),
        ];
        cfg.plots.cv_order = vec![];
        cfg.plots.dv_order = vec!["d2".to_string()];
        cfg.plots.custom_plots = vec![
            CustomPlotConfig {
                title: " ".to_string(),
                traces: vec![
                    CustomPlotTraceConfig {
                        direction: "input".to_string(),
                        signal: "cv.y1.process_value".to_string(),
                    },
                    CustomPlotTraceConfig {
                        direction: "input".to_string(),
                        signal: "cv.y1.process_value".to_string(),
                    },
                ],
            },
            CustomPlotConfig {
                title: "Empty".to_string(),
                traces: vec![CustomPlotTraceConfig {
                    direction: " ".to_string(),
                    signal: "".to_string(),
                }],
            },
        ];

        cfg.normalize_plot_config();

        assert_eq!(cfg.plots.window_sec, MIN_PLOT_WINDOW_SEC);
        assert_eq!(cfg.plots.future_window_sec, MAX_PLOT_FUTURE_WINDOW_SEC);
        assert_eq!(cfg.plots.mv_order, vec!["u2", "u1"]);
        assert_eq!(cfg.plots.cv_order, vec!["y1"]);
        assert_eq!(cfg.plots.dv_order, vec!["d2", "d1"]);
        assert_eq!(cfg.plots.custom_plots.len(), 1);
        assert_eq!(cfg.plots.custom_plots[0].title, "Custom Plot 1");
        assert_eq!(cfg.plots.custom_plots[0].traces.len(), 1);
    }
}
