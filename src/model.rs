use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use crate::types::SignalValue;

#[derive(Debug, Clone, Serialize)]
pub struct ModelMetadata {
    pub kind: String,
    pub description: String,
}

pub trait ModelModule: Send + Sync {
    fn metadata(&self) -> ModelMetadata;
    fn init(&mut self, initial_state: HashMap<String, SignalValue>);
    fn simulate_step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
    ) -> Result<HashMap<String, SignalValue>>;
}

#[derive(Debug, Default)]
pub struct NoopModel {
    _state: HashMap<String, SignalValue>,
}

impl ModelModule for NoopModel {
    fn metadata(&self) -> ModelMetadata {
        ModelMetadata {
            kind: "noop".to_string(),
            description: "No model configured".to_string(),
        }
    }

    fn init(&mut self, initial_state: HashMap<String, SignalValue>) {
        self._state = initial_state;
    }

    fn simulate_step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
    ) -> Result<HashMap<String, SignalValue>> {
        Ok(inputs.clone())
    }
}
