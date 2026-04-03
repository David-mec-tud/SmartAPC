use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::types::{SignalType, SignalValue};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub communication: CommunicationConfig,
    pub watchdog: WatchdogConfig,
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

    pub fn sidebar_tasks(&self) -> Vec<SidebarTask> {
        vec![
            SidebarTask::new(
                "watchdog",
                "Watchdog definieren",
                !matches!(self.watchdog, WatchdogConfig::None),
            ),
            SidebarTask::new(
                "io",
                "Ein- und Ausgänge definieren",
                !self.io.inputs.is_empty() && !self.io.outputs.is_empty(),
            ),
            SidebarTask::new(
                "limits",
                "Minima/Maxima für MV/CV festlegen",
                !self.limits.values.is_empty(),
            ),
            SidebarTask::new(
                "model",
                "Modell hinterlegen",
                self.model
                    .as_ref()
                    .map(|m| !m.kind.is_empty())
                    .unwrap_or(false),
            ),
            SidebarTask::new(
                "logic",
                "Regellogik hinterlegen",
                !self.logic.mapping_rules.is_empty(),
            ),
            SidebarTask::new(
                "params",
                "Parameter und Initialwerte festlegen",
                !self.logic.params.is_empty() || !self.logic.initial_values.is_empty(),
            ),
            SidebarTask::new(
                "timing",
                "Ausführungszeittest konfigurieren",
                self.runtime.execution_time_test,
            ),
            SidebarTask::new("simulation", "Reglersimulation", true),
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
}

impl Default for CommunicationConfig {
    fn default() -> Self {
        Self {
            protocol: ProtocolKind::OpcUa,
            endpoint_url: "opc.tcp://127.0.0.1:4840".to_string(),
        }
    }
}

impl CommunicationConfig {
    fn validate(&self) -> Result<()> {
        if self.endpoint_url.trim().is_empty() {
            bail!("communication.endpoint_url must not be empty");
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
}
