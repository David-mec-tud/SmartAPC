use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::types::{SignalType, SignalValue};

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
            runtime: RuntimeConfig::default(),
            alarms: AlarmConfig::default(),
            plots: PlotConfig::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.communication.validate()?;
        self.variables.validate()?;
        self.io_definition.validate(&self.variables)?;
        self.io.validate()?;
        self.limits.validate()?;
        self.logic.validate(&self.io)?;
        if let Some(model) = &self.model {
            model.validate(&self.variables)?;
        }
        self.plots.validate()?;

        if self.runtime.cycle_ms < 1_000 {
            bail!("runtime.cycle_ms must be >= 1000 ms for V1");
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
            mirror_cycle_ms,
            stale_timeout_ms,
        } = &self.watchdog
        {
            if read_node.trim().is_empty() || write_node.trim().is_empty() {
                bail!("watchdog counter_mirror requires read_node and write_node");
            }
            if *mirror_cycle_ms == 0 {
                bail!("watchdog mirror_cycle_ms must be > 0");
            }
            if *stale_timeout_ms == 0 {
                bail!("watchdog stale_timeout_ms must be > 0");
            }
        }

        Ok(())
    }

    pub fn normalize_plot_config(&mut self) {
        self.plots.window_sec = self
            .plots
            .window_sec
            .clamp(MIN_PLOT_WINDOW_SEC, MAX_PLOT_WINDOW_SEC);
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

    pub fn rebuild_runtime_io_from_definition(&mut self) -> Result<()> {
        let (io, limits) = self
            .io_definition
            .to_runtime_io_and_limits(&self.variables)?;
        self.io = io;
        self.limits = limits;
        Ok(())
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
            if cv.sollband_opc.as_deref().unwrap_or("").trim().is_empty() {
                if let Some(value) = cv.sollband_value {
                    defaults.insert(
                        format!("io.cv.{}.sollband", cv.name),
                        LogicParamValue::F64(value),
                    );
                }
            }
        }

        defaults
    }

    pub fn sidebar_tasks(&self) -> Vec<SidebarTask> {
        let legacy_io_present = !self.io.inputs.is_empty() || !self.io.outputs.is_empty();
        let variables_configured = self.variables.has_any() || legacy_io_present;
        let io_definition_configured = if self.variables.has_any() {
            self.io_definition.is_complete_for(&self.variables)
        } else {
            !self.io.inputs.is_empty() && !self.io.outputs.is_empty()
        };

        vec![
            SidebarTask::new(
                "communication",
                "OPC-UA Kommunikation konfigurieren",
                !self.communication.endpoint_url.trim().is_empty()
                    && self.communication.io_cycle_ms > 0,
            ),
            SidebarTask::new(
                "watchdog",
                "Watchdog definieren",
                !matches!(self.watchdog, WatchdogConfig::None),
            ),
            SidebarTask::new("io", "Ein- und Ausgänge definieren", variables_configured),
            SidebarTask::new("limits", "I/O definieren", io_definition_configured),
            SidebarTask::new("model", "Define Model (optional)", true),
            SidebarTask::new(
                "logic",
                "Regellogik hinterlegen",
                self.logic.is_configured_for(&self.io),
            ),
            SidebarTask::new(
                "timing",
                "Control Function Tests",
                self.logic.is_configured_for(&self.io),
            ),
            SidebarTask::new("simulation", "Reglersimulation (optional)", true),
            SidebarTask::new(
                "deployment",
                "Deployment konfigurieren",
                !self.deployment.service_name.is_empty(),
            ),
        ]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlotConfig {
    #[serde(default = "default_plot_window_sec")]
    pub window_sec: u32,
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
    #[serde(default = "default_communication_io_cycle_ms")]
    pub io_cycle_ms: u64,
}

impl Default for CommunicationConfig {
    fn default() -> Self {
        Self {
            protocol: ProtocolKind::OpcUa,
            endpoint_url: "opc.tcp://127.0.0.1:4840".to_string(),
            io_cycle_ms: default_communication_io_cycle_ms(),
        }
    }
}

impl CommunicationConfig {
    fn validate(&self) -> Result<()> {
        if self.endpoint_url.trim().is_empty() {
            bail!("communication.endpoint_url must not be empty");
        }
        if self.io_cycle_ms == 0 {
            bail!("communication.io_cycle_ms must be > 0");
        }
        Ok(())
    }
}

const fn default_communication_io_cycle_ms() -> u64 {
    1_000
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
        #[serde(default = "default_watchdog_mirror_cycle_ms")]
        mirror_cycle_ms: u64,
        #[serde(default = "default_watchdog_stale_timeout_ms")]
        stale_timeout_ms: u64,
    },
}

const fn default_watchdog_mirror_cycle_ms() -> u64 {
    1_000
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
                    bail!("mv '{}' has ll > hl", row.name);
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
            bail!("io_definition is incomplete for configured variables");
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

            if let Some(addr) = optional_trimmed(&row.soll) {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.soll", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }
            if let Some(addr) = optional_trimmed(&row.ist) {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.ist", name),
                    signal_type: SignalType::F64,
                    address: addr,
                    opc_tag: None,
                });
            }

            if let Some(sollband_opc) = optional_trimmed(row.sollband_opc.as_deref().unwrap_or(""))
            {
                inputs.push(SignalPoint {
                    name: format!("cv.{}.sollband", name),
                    signal_type: SignalType::F64,
                    address: sollband_opc,
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
            IoConfig { inputs, outputs },
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
    pub soll: String,
    #[serde(default)]
    pub ist: String,
    #[serde(default)]
    pub sollband_opc: Option<String>,
    #[serde(default)]
    pub sollband_value: Option<f64>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IoConfig {
    pub inputs: Vec<SignalPoint>,
    pub outputs: Vec<SignalPoint>,
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
                bail!("p_controller pair requires non-empty cv and mv");
            }
            let key = format!("{}->{}", pair.cv.trim(), pair.mv.trim());
            if !seen.insert(key) {
                bail!("duplicate p_controller pair '{}->{}'", pair.cv, pair.mv);
            }
            if !pair.kp.is_finite() || !pair.bias.is_finite() {
                bail!(
                    "p_controller pair '{}->{}' has non-finite kp/bias",
                    pair.cv,
                    pair.mv
                );
            }

            if !io.inputs.is_empty() || !io.outputs.is_empty() {
                let soll = format!("cv.{}.soll", pair.cv);
                let ist = format!("cv.{}.ist", pair.cv);
                let mv_write = format!("mv.{}.write", pair.mv);
                if !input_set.contains(soll.as_str()) {
                    bail!("p_controller pair references missing input '{}'", soll);
                }
                if !input_set.contains(ist.as_str()) {
                    bail!("p_controller pair references missing input '{}'", ist);
                }
                if !output_set.contains(mv_write.as_str()) {
                    bail!("p_controller pair references missing output '{}'", mv_write);
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
            let soll = format!("cv.{}.soll", pair.cv);
            let ist = format!("cv.{}.ist", pair.cv);
            let mv_write = format!("mv.{}.write", pair.mv);
            inputs.contains(soll.as_str())
                && inputs.contains(ist.as_str())
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
                    bail!("custom logic requires unknown input '{}'", required);
                }
            }
            for required in &self.required_outputs {
                if !output_set.contains(required.as_str()) {
                    bail!("custom logic requires unknown output '{}'", required);
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
                bail!("custom logic cannot be compatible without runtime outputs");
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

fn resize_matrix_overlap(matrix: Vec<Vec<f64>>, rows: usize, cols: usize) -> Vec<Vec<f64>> {
    let mut resized = vec![vec![0.0; cols]; rows];

    for (row_index, row) in matrix.into_iter().enumerate().take(rows) {
        for (col_index, value) in row.into_iter().enumerate().take(cols) {
            resized[row_index][col_index] = value;
        }
    }

    resized
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
    #[serde(default = "default_cycle_ms")]
    pub cycle_ms: u64,
    #[serde(default)]
    pub execution_time_test: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            cycle_ms: default_cycle_ms(),
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
    let mut cfg: ProjectConfig = toml::from_str(input).context("failed to parse project TOML")?;
    cfg.normalize_model_config();
    cfg.validate()?;
    Ok(cfg)
}

pub fn to_toml_string(cfg: &ProjectConfig) -> Result<String> {
    let mut normalized = cfg.clone();
    normalized.normalize_plot_config();
    normalized.normalize_model_config();
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
            name: "cv.y1.soll".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_SOL".to_string(),
            opc_tag: None,
        });
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=U1_WRITE".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.ist".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_IST".to_string(),
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
        cfg.runtime.cycle_ms = 10;
        let err = cfg.validate().expect_err("expected cycle validation error");
        assert!(err.to_string().contains("cycle_ms"));
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
  { name = "cv.y1.soll", signal_type = "f64", address = "ns=2;s=Y1_SOL" },
  { name = "cv.y1.ist", signal_type = "f64", address = "ns=2;s=Y1_IST" }
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

        let encoded = to_toml_string(&decoded).expect("toml encode");
        assert!(!encoded.contains("[model]"));
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
        assert!(simulation_task.label.contains("optional"));
        assert!(simulation_task.configured);
    }

    #[test]
    fn sidebar_uses_new_io_tasks_and_no_params_step() {
        let cfg = ProjectConfig::new("demo");
        let tasks = cfg.sidebar_tasks();

        assert!(
            tasks
                .iter()
                .any(|task| { task.id == "io" && task.label == "Ein- und Ausgänge definieren" })
        );
        assert!(
            tasks
                .iter()
                .any(|task| task.id == "limits" && task.label == "I/O definieren")
        );
        assert!(!tasks.iter().any(|task| task.id == "params"));
    }

    #[test]
    fn custom_logic_compatibility_requires_exact_io_match() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.soll".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_SOLL".to_string(),
            opc_tag: None,
        });
        cfg.io.inputs.push(SignalPoint {
            name: "cv.y1.ist".to_string(),
            signal_type: SignalType::F64,
            address: "ns=2;s=Y1_IST".to_string(),
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
        cfg.logic.custom_rust.required_inputs = vec!["cv.y1.soll".to_string()];
        cfg.logic.custom_rust.required_outputs = vec!["mv.u1.write".to_string()];
        cfg.logic.custom_rust.compatible = true;

        assert!(!cfg.logic.custom_rust.is_compatible_with(&cfg.io));

        cfg.logic.custom_rust.required_inputs =
            vec!["cv.y1.soll".to_string(), "cv.y1.ist".to_string()];
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
    fn normalize_plot_config_clamps_window_and_orders() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables.mvs = vec!["u1".to_string(), "u2".to_string()];
        cfg.variables.cvs = vec!["y1".to_string()];
        cfg.variables.dvs = vec!["d1".to_string(), "d2".to_string()];

        cfg.plots.window_sec = 8;
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
                        signal: "cv.y1.ist".to_string(),
                    },
                    CustomPlotTraceConfig {
                        direction: "input".to_string(),
                        signal: "cv.y1.ist".to_string(),
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
        assert_eq!(cfg.plots.mv_order, vec!["u2", "u1"]);
        assert_eq!(cfg.plots.cv_order, vec!["y1"]);
        assert_eq!(cfg.plots.dv_order, vec!["d2", "d1"]);
        assert_eq!(cfg.plots.custom_plots.len(), 1);
        assert_eq!(cfg.plots.custom_plots[0].title, "Custom Plot 1");
        assert_eq!(cfg.plots.custom_plots[0].traces.len(), 1);
    }
}
