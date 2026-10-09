//! Device handshake, independent of the controller lock and calculation worker.
//! Readiness expires at the end of the next scheduled suggestion window. A late
//! calculation cannot reassert flags cleared by the deadline monitor.
use crate::{communication::ProtocolAdapter, project_config::DeploymentConfig, types::SignalValue};
use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize)]
pub struct ControllerStatus {
    pub is_ready: bool,
    pub has_control: bool,
    pub enabled: bool,
    pub external_enable: bool,
    pub observer_converged: bool,
    pub reason: String,
}
struct State {
    status: ControllerStatus,
    deadline: Option<Instant>,
    generation: u64,
    stopped: bool,
}
pub struct ControllerSignals {
    state: Mutex<State>,
    // Serialize publication and actuator writes, including deadline revocation.
    pub exchange: tokio::sync::Mutex<()>,
    config: DeploymentConfig,
    protocol: Arc<dyn ProtocolAdapter>,
    timeout: Duration,
}
impl ControllerSignals {
    pub fn new(
        config: DeploymentConfig,
        protocol: Arc<dyn ProtocolAdapter>,
        cycle_ms: u64,
    ) -> Self {
        let external_enable = config
            .signal_nodes()
            .iter()
            .any(|(name, _)| *name == "ExternalControllerEnable");
        Self {
            state: Mutex::new(State {
                status: ControllerStatus {
                    is_ready: false,
                    has_control: false,
                    enabled: false,
                    external_enable,
                    observer_converged: false,
                    reason: "Waiting for the first successful suggestion".into(),
                },
                deadline: None,
                generation: 0,
                stopped: false,
            }),
            exchange: tokio::sync::Mutex::new(()),
            config,
            protocol,
            timeout: Duration::from_millis(cycle_ms.max(1)),
        }
    }
    pub fn is_stopped(&self) -> bool {
        self.state.lock().stopped
    }
    pub async fn stop(&self) -> Result<()> {
        {
            let mut state = self.state.lock();
            state.stopped = true;
            state.status.enabled = false;
        }
        self.revoke("Runtime stopped").await
    }
    pub fn snapshot(&self) -> ControllerStatus {
        self.state.lock().status.clone()
    }
    pub fn generation(&self) -> u64 {
        self.state.lock().generation
    }
    pub fn arm(&self, deadline: Instant) {
        self.state.lock().deadline = Some(deadline);
    }
    pub fn fault(&self, reason: impl Into<String>) {
        let mut state = self.state.lock();
        state.generation += 1;
        state.status.is_ready = false;
        state.status.has_control = false;
        state.status.reason = reason.into();
        state.deadline = None;
    }
    pub fn expire(&self, now: Instant) -> bool {
        let mut state = self.state.lock();
        if state.deadline.is_some_and(|deadline| now >= deadline) {
            state.generation += 1;
            state.status.is_ready = false;
            state.status.has_control = false;
            state.status.reason = "Control suggestion deadline missed".into();
            state.deadline = None;
            true
        } else {
            false
        }
    }
    pub fn suggestion(
        &self,
        generation: u64,
        converged: bool,
        successful: bool,
        next_deadline: Instant,
    ) {
        let mut state = self.state.lock();
        if state.stopped || state.generation != generation {
            return;
        }
        state.status.observer_converged = converged;
        state.status.is_ready = converged && successful;
        if !state.status.is_ready {
            state.status.has_control = false;
        }
        state.status.reason = if !converged {
            "Waiting for observer convergence"
        } else if !successful {
            "No successful control suggestion"
        } else {
            "Ready"
        }
        .into();
        state.deadline = Some(next_deadline);
    }
    pub fn mark_control(&self, written: bool) {
        let mut state = self.state.lock();
        state.status.has_control = written && state.status.enabled && state.status.is_ready;
    }
    pub fn set_enabled(&self, enabled: bool, external: bool) -> Result<()> {
        let mut state = self.state.lock();
        if state.stopped {
            bail!("runtime is stopping");
        }
        if state.status.external_enable != external {
            bail!("Controller enable is owned by ExternalControllerEnable");
        }
        state.status.enabled = enabled;
        if !enabled {
            state.status.has_control = false;
        }
        Ok(())
    }
    pub async fn read_enable(&self) -> Result<()> {
        if let Some((_, node)) = self
            .config
            .signal_nodes()
            .into_iter()
            .find(|(name, _)| *name == "ExternalControllerEnable")
        {
            let value = tokio::time::timeout(self.timeout, self.protocol.read(node))
                .await
                .context("ExternalControllerEnable read timed out")??;
            let SignalValue::Bool(enabled) = value else {
                bail!("ExternalControllerEnable must be an OPC UA Boolean");
            };
            self.set_enabled(enabled, true)?;
        }
        Ok(())
    }
    /// Caller holds `exchange`. Attempt both flags even if one write fails.
    pub async fn publish(&self) -> Result<()> {
        let status = self.snapshot();
        let mut error = None;
        for (name, node) in self.config.signal_nodes() {
            let value = match name {
                "ControllerIsReady" => status.is_ready,
                "ControllerHasControl" => status.has_control,
                _ => continue,
            };
            let result = tokio::time::timeout(
                self.timeout,
                self.protocol.write(node, SignalValue::Bool(value)),
            )
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    error = Some(err.context(format!("failed writing {name}")));
                }
                Err(err) => {
                    error = Some(anyhow::anyhow!("{name} write timed out: {err}"));
                }
            }
        }
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(())
        }
    }
    pub async fn revoke(&self, reason: impl Into<String>) -> Result<()> {
        self.fault(reason);
        let _exchange = self.exchange.lock().await;
        self.publish().await
    }
    pub async fn monitor(self: Arc<Self>) {
        let mut timer = tokio::time::interval(Duration::from_millis(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if self.expire(Instant::now()) {
                let _exchange = self.exchange.lock().await;
                if let Err(err) = self.publish().await {
                    tracing::error!(error = %err, "failed clearing controller flags after deadline");
                }
            }
        }
    }
}
