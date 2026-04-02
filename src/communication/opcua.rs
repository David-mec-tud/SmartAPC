use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use async_opcua::{
    client::{ClientBuilder, IdentityToken, Session},
    types::{
        EndpointDescription, MessageSecurityMode, NodeId, ReadValueId, StatusCode,
        TimestampsToReturn, UserTokenPolicy, Variant, WriteValue,
    },
};
use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};

use crate::{communication::HealthStatus, types::SignalValue};

use super::ProtocolAdapter;

struct RealClientState {
    session: Arc<Session>,
    event_loop_handle: tokio::task::JoinHandle<StatusCode>,
}

enum Backend {
    Memory {
        nodes: RwLock<HashMap<String, SignalValue>>,
    },
    Real {
        state: Mutex<Option<RealClientState>>,
    },
}

/// V1 OPC-UA client adapter.
///
/// - `opc.tcp://...` endpoints use a real OPC-UA client transport.
/// - `mock://...` endpoints keep an in-memory node store for tests/offline runs.
pub struct OpcUaClientAdapter {
    endpoint_url: String,
    connected: AtomicBool,
    force_fault: AtomicBool,
    backend: Backend,
}

impl std::fmt::Debug for OpcUaClientAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcUaClientAdapter")
            .field("endpoint_url", &self.endpoint_url)
            .field("connected", &self.connected.load(Ordering::SeqCst))
            .finish()
    }
}

impl OpcUaClientAdapter {
    pub fn new(endpoint_url: impl Into<String>) -> Self {
        let endpoint_url = endpoint_url.into();
        let backend = if endpoint_url.starts_with("mock://") {
            Backend::Memory {
                nodes: RwLock::new(HashMap::new()),
            }
        } else {
            Backend::Real {
                state: Mutex::new(None),
            }
        };

        Self {
            endpoint_url,
            connected: AtomicBool::new(false),
            force_fault: AtomicBool::new(false),
            backend,
        }
    }

    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    pub fn seed_node(&self, node: impl Into<String>, value: SignalValue) {
        if let Backend::Memory { nodes } = &self.backend {
            nodes.write().insert(node.into(), value);
        }
    }

    pub fn set_force_fault(&self, value: bool) {
        self.force_fault.store(value, Ordering::SeqCst);
    }

    fn ensure_online(&self) -> anyhow::Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            bail!("OPC-UA client is not connected");
        }
        if self.force_fault.load(Ordering::SeqCst) {
            bail!("OPC-UA client is in forced fault mode");
        }
        Ok(())
    }

    fn parse_node_id(node: &str) -> anyhow::Result<NodeId> {
        NodeId::from_str(node).map_err(|status| anyhow!("invalid node id '{node}': {status:?}"))
    }

    fn signal_to_variant(value: SignalValue) -> Variant {
        match value {
            SignalValue::F64(v) => Variant::from(v),
            SignalValue::Bool(v) => Variant::from(v),
        }
    }

    fn variant_to_signal(value: Variant) -> anyhow::Result<SignalValue> {
        match value {
            Variant::Boolean(v) => Ok(SignalValue::Bool(v)),
            v => v
                .as_f64()
                .map(SignalValue::F64)
                .ok_or_else(|| anyhow!("unsupported OPC-UA variant for signal conversion")),
        }
    }

    fn real_session(&self) -> anyhow::Result<Arc<Session>> {
        let lock = match &self.backend {
            Backend::Real { state } => state,
            Backend::Memory { .. } => bail!("real session requested on mock backend"),
        };

        let guard = lock.lock();
        let Some(state) = guard.as_ref() else {
            bail!("OPC-UA client session is not initialized");
        };

        if state.event_loop_handle.is_finished() {
            bail!("OPC-UA session event loop has terminated");
        }

        Ok(Arc::clone(&state.session))
    }
}

#[async_trait]
impl ProtocolAdapter for OpcUaClientAdapter {
    async fn connect(&self) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Memory { .. } => {
                self.connected.store(true, Ordering::SeqCst);
                Ok(())
            }
            Backend::Real { state } => {
                if let Some(existing) = state.lock().as_ref() {
                    if !existing.event_loop_handle.is_finished() {
                        self.connected.store(true, Ordering::SeqCst);
                        return Ok(());
                    }
                }

                let pki_dir = std::env::temp_dir().join("smartapc_opcua_client_pki");
                std::fs::create_dir_all(&pki_dir).with_context(|| {
                    format!("failed creating client pki dir {}", pki_dir.display())
                })?;

                let mut client = ClientBuilder::new()
                    .application_name("SmartAPC OPCUA Client")
                    .application_uri("urn:smartapc:opcua-client")
                    .product_uri("urn:smartapc")
                    .create_sample_keypair(true)
                    .pki_dir(&pki_dir)
                    .trust_server_certs(true)
                    .verify_server_certs(false)
                    .session_retry_limit(1)
                    .client()
                    .map_err(|errs| {
                        anyhow!("failed to build OPC-UA client: {}", errs.join(", "))
                    })?;

                let endpoint: EndpointDescription = (
                    self.endpoint_url.as_str(),
                    "None",
                    MessageSecurityMode::None,
                    UserTokenPolicy::anonymous(),
                )
                    .into();

                let (session, event_loop) = client
                    .connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous)
                    .await
                    .map_err(|e| anyhow!("failed to connect OPC-UA session: {e:?}"))?;

                let event_loop_handle = event_loop.spawn();

                let connected =
                    tokio::time::timeout(Duration::from_secs(5), session.wait_for_connection())
                        .await
                        .unwrap_or(false);
                if !connected {
                    event_loop_handle.abort();
                    bail!("OPC-UA session failed to reach connected state");
                }

                let mut guard = state.lock();
                if let Some(old) = guard.replace(RealClientState {
                    session,
                    event_loop_handle,
                }) {
                    old.event_loop_handle.abort();
                }

                self.connected.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    async fn read(&self, node: &str) -> anyhow::Result<SignalValue> {
        self.ensure_online()?;

        match &self.backend {
            Backend::Memory { nodes } => nodes
                .read()
                .get(node)
                .cloned()
                .ok_or_else(|| anyhow!("node '{}' not found", node)),
            Backend::Real { .. } => {
                let session = self.real_session()?;
                let node_id = Self::parse_node_id(node)?;
                let read_values = [ReadValueId::from(node_id)];

                let values = session
                    .read(&read_values, TimestampsToReturn::Neither, 0.0)
                    .await
                    .map_err(|status| anyhow!("OPC-UA read request failed: {status:?}"))?;

                let value = values
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow!("OPC-UA read returned no values"))?;

                if let Some(status) = value.status {
                    if status.is_bad() {
                        bail!("OPC-UA read value status is bad: {status:?}");
                    }
                }

                let variant = value
                    .value
                    .ok_or_else(|| anyhow!("OPC-UA read value has no payload"))?;
                Self::variant_to_signal(variant)
            }
        }
    }

    async fn write(&self, node: &str, value: SignalValue) -> anyhow::Result<()> {
        self.ensure_online()?;

        match &self.backend {
            Backend::Memory { nodes } => {
                nodes.write().insert(node.to_string(), value);
                Ok(())
            }
            Backend::Real { .. } => {
                let session = self.real_session()?;
                let node_id = Self::parse_node_id(node)?;

                let statuses = session
                    .write(&[WriteValue::value_attr(
                        node_id,
                        Self::signal_to_variant(value),
                    )])
                    .await
                    .map_err(|status| anyhow!("OPC-UA write request failed: {status:?}"))?;

                let status = statuses
                    .first()
                    .copied()
                    .ok_or_else(|| anyhow!("OPC-UA write returned no status"))?;

                if status.is_bad() {
                    bail!("OPC-UA write was rejected: {status:?}");
                }

                Ok(())
            }
        }
    }

    fn health(&self) -> HealthStatus {
        if self.force_fault.load(Ordering::SeqCst) {
            return HealthStatus::Unhealthy;
        }

        match &self.backend {
            Backend::Memory { .. } => {
                if self.connected.load(Ordering::SeqCst) {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                }
            }
            Backend::Real { state } => {
                if !self.connected.load(Ordering::SeqCst) {
                    return HealthStatus::Unhealthy;
                }

                let guard = state.lock();
                if let Some(session) = guard.as_ref() {
                    if !session.event_loop_handle.is_finished() {
                        HealthStatus::Healthy
                    } else {
                        HealthStatus::Unhealthy
                    }
                } else {
                    HealthStatus::Unhealthy
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Result;
    use async_opcua::{
        crypto::SecurityPolicy,
        server::{
            ANONYMOUS_USER_TOKEN_ID, ServerBuilder,
            address_space::{AccessLevel, Variable},
            diagnostics::NamespaceMetadata,
            node_manager::memory::{SimpleNodeManager, simple_node_manager},
        },
        types::{DataValue, MessageSecurityMode, NodeId, ObjectId},
    };
    use tempfile::TempDir;

    use super::*;

    struct FakePlcServer {
        endpoint_url: String,
        pv_node: String,
        mv_node: String,
        pump_node: String,
        wd_counter_node: String,
        wd_mirror_node: String,
        handle: async_opcua::server::ServerHandle,
        server_task: tokio::task::JoinHandle<Result<(), String>>,
        plc_task: tokio::task::JoinHandle<()>,
        _tmp_dir: TempDir,
    }

    impl FakePlcServer {
        async fn start() -> Result<Self> {
            let tmp_dir = tempfile::tempdir().context("failed creating fake PLC temp dir")?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .context("failed binding fake PLC listener")?;
            let port = listener
                .local_addr()
                .context("failed getting fake PLC listener addr")?
                .port();

            let namespace_uri = "urn:smartapc:fake-plc";

            let (server, handle) = ServerBuilder::new()
                .application_name("SmartAPC Fake PLC")
                .application_uri("urn:smartapc:fake-plc:server")
                .product_uri("urn:smartapc:fake-plc")
                .host("127.0.0.1")
                .port(port)
                .create_sample_keypair(true)
                .pki_dir(tmp_dir.path().join("server-pki"))
                .discovery_urls(vec!["/".to_string()])
                .add_endpoint(
                    "none",
                    (
                        "/",
                        SecurityPolicy::None,
                        MessageSecurityMode::None,
                        &[ANONYMOUS_USER_TOKEN_ID] as &[&str],
                    ),
                )
                .with_node_manager(simple_node_manager(
                    NamespaceMetadata {
                        namespace_uri: namespace_uri.to_string(),
                        ..Default::default()
                    },
                    "fake_plc",
                ))
                .build()
                .map_err(anyhow::Error::msg)?;

            let node_manager = handle
                .node_managers()
                .get_of_type::<SimpleNodeManager>()
                .ok_or_else(|| anyhow!("fake PLC node manager not found"))?;

            let ns = handle
                .get_namespace_index(namespace_uri)
                .ok_or_else(|| anyhow!("fake PLC namespace index not found"))?;

            let folder_id = NodeId::new(ns, "SmartAPC");
            let pv_id = NodeId::new(ns, "PV");
            let mv_id = NodeId::new(ns, "MV");
            let pump_id = NodeId::new(ns, "PumpOn");
            let wd_counter_id = NodeId::new(ns, "WD.Counter");
            let wd_mirror_id = NodeId::new(ns, "WD.Mirror");

            {
                let mut mv_var = Variable::new(&mv_id, "MV", "MV", 0.0f64);
                mv_var.set_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
                mv_var
                    .set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);

                let mut pump_var = Variable::new(&pump_id, "PumpOn", "PumpOn", false);
                pump_var.set_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
                pump_var
                    .set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);

                let mut wd_mirror_var =
                    Variable::new(&wd_mirror_id, "WD.Mirror", "WD.Mirror", 0.0f64);
                wd_mirror_var
                    .set_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
                wd_mirror_var
                    .set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);

                let mut address_space = node_manager.address_space().write();
                address_space.add_folder(
                    &folder_id,
                    "SmartAPC",
                    "SmartAPC",
                    &ObjectId::ObjectsFolder.into(),
                );
                let _ = address_space.add_variables(
                    vec![
                        Variable::new(&pv_id, "PV", "PV", 0.0f64),
                        mv_var,
                        pump_var,
                        Variable::new(&wd_counter_id, "WD.Counter", "WD.Counter", 0.0f64),
                        wd_mirror_var,
                    ],
                    &folder_id,
                );
            }

            let subscriptions = Arc::clone(handle.subscriptions());
            let token = handle.token().clone();
            let plc_manager = Arc::clone(&node_manager);
            let wd_counter_clone = wd_counter_id.clone();
            let pv_clone = pv_id.clone();
            let plc_task = tokio::spawn(async move {
                let mut counter = 0.0f64;
                while !token.is_cancelled() {
                    counter += 1.0;
                    let _ = plc_manager.set_value(
                        subscriptions.as_ref(),
                        &wd_counter_clone,
                        None,
                        DataValue::new_now(counter),
                    );
                    let _ = plc_manager.set_value(
                        subscriptions.as_ref(),
                        &pv_clone,
                        None,
                        DataValue::new_now(counter * 0.5),
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });

            let server_task = tokio::spawn(async move { server.run_with(listener).await });

            Ok(Self {
                endpoint_url: format!("opc.tcp://127.0.0.1:{port}/"),
                pv_node: pv_id.to_string(),
                mv_node: mv_id.to_string(),
                pump_node: pump_id.to_string(),
                wd_counter_node: wd_counter_id.to_string(),
                wd_mirror_node: wd_mirror_id.to_string(),
                handle,
                server_task,
                plc_task,
                _tmp_dir: tmp_dir,
            })
        }

        async fn shutdown(self) -> Result<()> {
            self.handle.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.server_task)
                .await
                .context("timed out waiting for fake PLC server shutdown")??;
            let _ = tokio::time::timeout(Duration::from_secs(2), self.plc_task)
                .await
                .context("timed out waiting for fake PLC loop shutdown")?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn mock_backend_read_write() {
        let adapter = OpcUaClientAdapter::new("mock://local");
        adapter.seed_node("ns=2;s=PV", SignalValue::F64(12.0));

        adapter.connect().await.expect("connect");
        let pv = adapter.read("ns=2;s=PV").await.expect("read pv");
        assert_eq!(pv, SignalValue::F64(12.0));

        adapter
            .write("ns=2;s=MV", SignalValue::F64(5.5))
            .await
            .expect("write mv");
        let mv = adapter.read("ns=2;s=MV").await.expect("read mv");
        assert_eq!(mv, SignalValue::F64(5.5));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn real_opcua_connects_to_fake_plc_server() {
        let fake_plc = FakePlcServer::start().await.expect("start fake PLC");

        let adapter = OpcUaClientAdapter::new(fake_plc.endpoint_url.clone());
        adapter.connect().await.expect("connect OPC-UA client");

        let c1 = adapter
            .read(&fake_plc.wd_counter_node)
            .await
            .expect("read watchdog counter #1");
        tokio::time::sleep(Duration::from_millis(250)).await;
        let c2 = adapter
            .read(&fake_plc.wd_counter_node)
            .await
            .expect("read watchdog counter #2");

        let c1 = c1.as_f64().expect("counter f64 #1");
        let c2 = c2.as_f64().expect("counter f64 #2");
        assert!(c2 > c1, "counter should increment in fake PLC");

        adapter
            .write(&fake_plc.mv_node, SignalValue::F64(42.0))
            .await
            .expect("write MV");
        let mv = adapter.read(&fake_plc.mv_node).await.expect("read MV");
        assert_eq!(mv, SignalValue::F64(42.0));

        adapter
            .write(&fake_plc.pump_node, SignalValue::Bool(true))
            .await
            .expect("write PumpOn");
        let pump = adapter
            .read(&fake_plc.pump_node)
            .await
            .expect("read PumpOn");
        assert_eq!(pump, SignalValue::Bool(true));

        let wd = adapter
            .read(&fake_plc.wd_counter_node)
            .await
            .expect("read watchdog for mirror");
        adapter
            .write(&fake_plc.wd_mirror_node, wd)
            .await
            .expect("write watchdog mirror");

        let pv = adapter.read(&fake_plc.pv_node).await.expect("read PV");
        assert!(pv.as_f64().expect("pv f64") > 0.0);

        fake_plc.shutdown().await.expect("shutdown fake PLC");
    }
}
