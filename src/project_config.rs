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
    pub model: Option<ModelConfig>,
    pub runtime: RuntimeConfig,
    pub alarms: AlarmConfig,
    pub deployment: DeploymentConfig,
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
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.communication.validate()?;
        self.variables.validate()?;
        self.io_definition.validate(&self.variables)?;
        self.io.validate()?;
        self.limits.validate()?;
        self.logic.validate(&self.io)?;

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

    pub fn rebuild_runtime_io_from_definition(&mut self) -> Result<()> {
        let (io, limits) = self
            .io_definition
            .to_runtime_io_and_limits(&self.variables)?;
        self.io = io;
        self.limits = limits;
        Ok(())
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
            SidebarTask::new("model", "Modell hinterlegen (optional)", true),
            SidebarTask::new(
                "logic",
                "Regellogik hinterlegen",
                !self.logic.mapping_rules.is_empty(),
            ),
            SidebarTask::new(
                "timing",
                "Ausführungszeittest konfigurieren",
                self.runtime.execution_time_test,
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
    pub kind: String,
    #[serde(default)]
    pub mapping_rules: Vec<MappingRule>,
    #[serde(default)]
    pub params: HashMap<String, f64>,
    #[serde(default)]
    pub initial_values: HashMap<String, SignalValue>,
}

impl Default for LogicConfig {
    fn default() -> Self {
        Self {
            kind: "mapping".to_string(),
            mapping_rules: Vec::new(),
            params: HashMap::new(),
            initial_values: HashMap::new(),
        }
    }
}

impl LogicConfig {
    fn validate(&self, io: &IoConfig) -> Result<()> {
        if self.kind.trim().is_empty() {
            bail!("logic.kind must not be empty");
        }

        let input_set: HashSet<_> = io.inputs.iter().map(|s| s.name.as_str()).collect();
        let output_set: HashSet<_> = io.outputs.iter().map(|s| s.name.as_str()).collect();

        for rule in &self.mapping_rules {
            if !input_set.contains(rule.input.as_str()) {
                bail!("logic rule references unknown input '{}'", rule.input);
            }
            if !output_set.contains(rule.output.as_str()) {
                bail!("logic rule references unknown output '{}'", rule.output);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingRule {
    pub input: String,
    pub output: String,
    #[serde(default = "default_gain")]
    pub gain: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub invert_bool: bool,
}

const fn default_gain() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub kind: String,
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
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
    let cfg: ProjectConfig = toml::from_str(input).context("failed to parse project TOML")?;
    cfg.validate()?;
    Ok(cfg)
}

pub fn to_toml_string(cfg: &ProjectConfig) -> Result<String> {
    cfg.validate()?;
    toml::to_string_pretty(cfg).context("failed to encode project TOML")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let mut cfg = ProjectConfig::new("demo");
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
        cfg.logic.mapping_rules.push(MappingRule {
            input: "pv".to_string(),
            output: "mv".to_string(),
            gain: 1.0,
            offset: 0.0,
            invert_bool: false,
        });

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
        let mut cfg = ProjectConfig::new("demo");
        cfg.io.inputs.push(SignalPoint {
            name: "in".to_string(),
            signal_type: SignalType::Bool,
            address: "ns=2;s=In".to_string(),
            opc_tag: Some("TAG_IN".to_string()),
        });
        cfg.io.outputs.push(SignalPoint {
            name: "out".to_string(),
            signal_type: SignalType::Bool,
            address: "ns=2;s=Out".to_string(),
            opc_tag: None,
        });
        cfg.logic.mapping_rules.push(MappingRule {
            input: "in".to_string(),
            output: "out".to_string(),
            gain: 1.0,
            offset: 0.0,
            invert_bool: false,
        });

        let encoded = to_toml_string(&cfg).expect("toml encode");
        let decoded = from_toml_str(&encoded).expect("toml decode");
        assert_eq!(decoded.name, "demo");
    }

    #[test]
    fn optional_model_and_simulation_do_not_block_sidebar_progress() {
        let mut cfg = ProjectConfig::new("demo");
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
        cfg.logic.mapping_rules.push(MappingRule {
            input: "pv".to_string(),
            output: "mv".to_string(),
            gain: 1.0,
            offset: 0.0,
            invert_bool: false,
        });

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

        assert!(model_task.label.contains("optional"));
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
}
