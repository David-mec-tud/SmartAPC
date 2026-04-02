use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use crate::{
    project_config::MappingRule,
    types::{RuntimeState, SignalValue},
};

#[derive(Debug, Clone, Serialize)]
pub struct LogicMetadata {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterSchemaEntry {
    pub name: String,
    pub description: String,
    pub default: f64,
}

pub trait ControlLogic: Send + Sync {
    fn metadata(&self) -> LogicMetadata;
    fn init(&mut self, initial_values: HashMap<String, SignalValue>, params: HashMap<String, f64>);
    fn step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        state: RuntimeState,
    ) -> Result<HashMap<String, SignalValue>>;
    fn parameter_schema(&self) -> Vec<ParameterSchemaEntry>;
    fn set_params(&mut self, params: HashMap<String, f64>);
    fn current_params(&self) -> HashMap<String, f64>;
}

#[derive(Debug, Clone)]
pub struct MappingLogic {
    rules: Vec<MappingRule>,
    params: HashMap<String, f64>,
    _initial_values: HashMap<String, SignalValue>,
}

impl MappingLogic {
    pub fn new(rules: Vec<MappingRule>, params: HashMap<String, f64>) -> Self {
        Self {
            rules,
            params,
            _initial_values: HashMap::new(),
        }
    }

    fn rule_gain(&self, rule: &MappingRule) -> f64 {
        self.params
            .get(&format!("{}.gain", rule.output))
            .copied()
            .unwrap_or(rule.gain)
    }

    fn rule_offset(&self, rule: &MappingRule) -> f64 {
        self.params
            .get(&format!("{}.offset", rule.output))
            .copied()
            .unwrap_or(rule.offset)
    }
}

impl ControlLogic for MappingLogic {
    fn metadata(&self) -> LogicMetadata {
        LogicMetadata {
            kind: "mapping".to_string(),
            description: "Static compiled mapping logic for V1".to_string(),
        }
    }

    fn init(&mut self, initial_values: HashMap<String, SignalValue>, params: HashMap<String, f64>) {
        self._initial_values = initial_values;
        self.set_params(params);
    }

    fn step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        _state: RuntimeState,
    ) -> Result<HashMap<String, SignalValue>> {
        let mut outputs = HashMap::new();

        for rule in &self.rules {
            if let Some(value) = inputs.get(&rule.input) {
                let output = match value {
                    SignalValue::F64(v) => {
                        let mapped = (*v * self.rule_gain(rule)) + self.rule_offset(rule);
                        SignalValue::F64(mapped)
                    }
                    SignalValue::Bool(v) => {
                        SignalValue::Bool(if rule.invert_bool { !v } else { *v })
                    }
                };
                outputs.insert(rule.output.clone(), output);
            }
        }

        Ok(outputs)
    }

    fn parameter_schema(&self) -> Vec<ParameterSchemaEntry> {
        let mut schema = Vec::new();
        for rule in &self.rules {
            schema.push(ParameterSchemaEntry {
                name: format!("{}.gain", rule.output),
                description: format!("Gain applied to mapping output {}", rule.output),
                default: rule.gain,
            });
            schema.push(ParameterSchemaEntry {
                name: format!("{}.offset", rule.output),
                description: format!("Offset applied to mapping output {}", rule.output),
                default: rule.offset,
            });
        }
        schema
    }

    fn set_params(&mut self, params: HashMap<String, f64>) {
        self.params.extend(params);
    }

    fn current_params(&self) -> HashMap<String, f64> {
        self.params.clone()
    }
}
