use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::SignalValue;

pub mod opcua;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
}

#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    async fn connect(&self) -> anyhow::Result<()>;
    async fn read(&self, node: &str) -> anyhow::Result<SignalValue>;
    async fn write(&self, node: &str, value: SignalValue) -> anyhow::Result<()>;
    fn health(&self) -> HealthStatus;
}
