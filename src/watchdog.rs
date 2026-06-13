use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use async_trait::async_trait;
use serde::Serialize;

use crate::{communication::ProtocolAdapter, types::SignalValue};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchdogStatus {
    Disabled,
    Ok,
    Fault { reason: String },
}

#[async_trait]
pub trait WatchdogModule: Send + Sync {
    async fn init(&mut self, protocol: &dyn ProtocolAdapter) -> anyhow::Result<()>;
    async fn tick(&mut self, protocol: &dyn ProtocolAdapter) -> anyhow::Result<WatchdogStatus>;
    fn status(&self) -> WatchdogStatus;
}

#[derive(Debug, Default)]
pub struct NoneWatchdog;

#[async_trait]
impl WatchdogModule for NoneWatchdog {
    async fn init(&mut self, _protocol: &dyn ProtocolAdapter) -> anyhow::Result<()> {
        Ok(())
    }

    async fn tick(&mut self, _protocol: &dyn ProtocolAdapter) -> anyhow::Result<WatchdogStatus> {
        Ok(WatchdogStatus::Disabled)
    }

    fn status(&self) -> WatchdogStatus {
        WatchdogStatus::Disabled
    }
}

#[derive(Debug)]
pub struct CounterMirrorWatchdog {
    read_node: String,
    write_node: String,
    stale_timeout: Duration,
    last_counter: Option<f64>,
    last_change: Instant,
    status: WatchdogStatus,
}

impl CounterMirrorWatchdog {
    pub fn new(
        read_node: impl Into<String>,
        write_node: impl Into<String>,
        stale_timeout: Duration,
    ) -> Self {
        Self {
            read_node: read_node.into(),
            write_node: write_node.into(),
            stale_timeout,
            last_counter: None,
            last_change: Instant::now(),
            status: WatchdogStatus::Ok,
        }
    }

    fn set_fault(&mut self, reason: impl Into<String>) -> WatchdogStatus {
        let reason = reason.into();
        let fault = WatchdogStatus::Fault { reason };
        self.status = fault.clone();
        fault
    }
}

#[async_trait]
impl WatchdogModule for CounterMirrorWatchdog {
    async fn init(&mut self, protocol: &dyn ProtocolAdapter) -> anyhow::Result<()> {
        let initial = protocol.read(&self.read_node).await?;
        let value = initial
            .as_f64()
            .ok_or_else(|| anyhow!("watchdog counter must be f64"))?;
        protocol
            .write(&self.write_node, SignalValue::F64(value))
            .await?;
        self.last_counter = Some(value);
        self.last_change = Instant::now();
        self.status = WatchdogStatus::Ok;
        Ok(())
    }

    async fn tick(&mut self, protocol: &dyn ProtocolAdapter) -> anyhow::Result<WatchdogStatus> {
        let value = match protocol.read(&self.read_node).await {
            Ok(v) => v,
            Err(err) => return Ok(self.set_fault(format!("watchdog read error: {err}"))),
        };

        let counter = match value.as_f64() {
            Some(v) => v,
            None => return Ok(self.set_fault("watchdog counter must be f64")),
        };

        if let Some(last) = self.last_counter {
            if (counter - last).abs() > f64::EPSILON {
                self.last_change = Instant::now();
                self.last_counter = Some(counter);
            }
        } else {
            self.last_change = Instant::now();
            self.last_counter = Some(counter);
        }

        if self.last_change.elapsed() > self.stale_timeout {
            return Ok(self.set_fault("watchdog stale counter timeout"));
        }

        if protocol
            .write(&self.write_node, SignalValue::F64(counter))
            .await
            .is_err()
        {
            return Ok(self.set_fault("watchdog mirror write failed"));
        }

        self.status = WatchdogStatus::Ok;
        Ok(WatchdogStatus::Ok)
    }

    fn status(&self) -> WatchdogStatus {
        self.status.clone()
    }
}

pub fn stale_timeout_or_default(ms: u64) -> anyhow::Result<Duration> {
    if ms == 0 {
        bail!("watchdog stale timeout must be > 0");
    }
    Ok(Duration::from_millis(ms))
}

#[cfg(test)]
mod tests {
    use crate::{
        communication::{ProtocolAdapter, opcua::OpcUaClientAdapter},
        types::SignalValue,
    };

    use super::{CounterMirrorWatchdog, WatchdogModule};

    #[tokio::test]
    async fn counter_mirror_writes_on_every_tick() {
        let adapter = OpcUaClientAdapter::new("mock://watchdog-cycle");
        adapter.connect().await.expect("connect");
        adapter.seed_node("ns=2;s=WD.Read", SignalValue::F64(1.0));
        adapter.seed_node("ns=2;s=WD.Write", SignalValue::F64(0.0));

        let mut watchdog = CounterMirrorWatchdog::new(
            "ns=2;s=WD.Read",
            "ns=2;s=WD.Write",
            std::time::Duration::from_secs(5),
        );

        watchdog.init(&adapter).await.expect("init");
        adapter.seed_node("ns=2;s=WD.Read", SignalValue::F64(2.0));

        watchdog.tick(&adapter).await.expect("first tick");
        let write_after_first = adapter
            .read("ns=2;s=WD.Write")
            .await
            .expect("read write tag after first tick");
        assert_eq!(write_after_first, SignalValue::F64(2.0));
    }
}
