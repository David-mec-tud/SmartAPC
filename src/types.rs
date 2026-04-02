use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Running,
    Hold,
    Alarm,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SignalValue {
    F64(f64),
    Bool(bool),
}

impl SignalValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            SignalValue::F64(v) => Some(*v),
            SignalValue::Bool(_) => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            SignalValue::Bool(v) => Some(*v),
            SignalValue::F64(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    F64,
    Bool,
}
