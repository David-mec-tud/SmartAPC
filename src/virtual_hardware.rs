use std::{
    collections::VecDeque,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use async_opcua::{
    crypto::SecurityPolicy,
    server::{
        ANONYMOUS_USER_TOKEN_ID, ServerBuilder,
        address_space::{AccessLevel, Variable},
        diagnostics::NamespaceMetadata,
        node_manager::memory::{SimpleNodeManager, simple_node_manager},
    },
    types::{AttributeId, DataEncoding, DataValue, MessageSecurityMode, NodeId, ObjectId},
};
use axum::{Json, Router, extract::State, response::Redirect, routing::get};
#[cfg(feature = "legacy-native")]
use eframe::egui;
#[cfg(feature = "legacy-native")]
use egui_plot::{Legend, Line, Plot, PlotPoints};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const Y_VALUE: &str = "y1_value";
const Y_TARGET: &str = "y1_target";
const Y_HL: &str = "y1_HL";
const Y_LL: &str = "y1_LL";
// Explicit offsets coexist with the legacy absolute display nodes without aliasing semantics.
const Y_BAND_LOW: &str = "y1_band_low";
const Y_BAND_HIGH: &str = "y1_band_high";
const U_WRITE: &str = "u1_write";
const U_READ: &str = "u1_read";
const WD_COUNTER: &str = "wd_counter_read";
const WD_MIRROR: &str = "wd_counter_mirror";

const CONTROLLER_READY: &str = "ControllerIsReady";
const CONTROLLER_CONTROL: &str = "ControllerHasControl";
const CONTROLLER_ENABLE: &str = "ExternalControllerEnable";

const PLOT_WINDOW_SECONDS: f64 = 180.0;
#[cfg(feature = "legacy-native")]
const PLOT_REFRESH_MS: u64 = 100;

#[derive(Debug, Clone, Serialize)]
pub struct VirtualHardwareConfig {
    pub host: String,
    pub port: u16,
    pub dt_ms: u64,
    pub tau_s: f64,
    pub gain: f64,
    pub history: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Sample {
    y: f64,
    u: f64,
    target: f64,
    hl: f64,
    ll: f64,
    band_low: f64,
    band_high: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct TimedSample {
    t_s: f64,
    sample: Sample,
}

#[derive(Debug, Clone, Copy)]
struct ExternalInputs {
    u_write: f64,
    target: f64,
    hl: f64,
    ll: f64,
}

impl Default for ExternalInputs {
    fn default() -> Self {
        Self {
            u_write: 0.0,
            target: 0.0,
            hl: 1.0,
            ll: -1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PendingManualWrites {
    external_controller_enable: Option<bool>,
    u_write: Option<f64>,
    target: Option<f64>,
    hl: Option<f64>,
    ll: Option<f64>,
    band_low: Option<f64>,
    band_high: Option<f64>,
}

/// Run the OPC UA test plant without a native window or a browser connection.
pub async fn run(config: VirtualHardwareConfig) -> Result<()> {
    run_with_feedback(config, Feedback::Headless).await
}

/// Serve the same locally packaged React/Plotly assets as the main application.
pub async fn run_browser(config: VirtualHardwareConfig, bind: SocketAddr) -> Result<()> {
    run_with_feedback(config, Feedback::Browser(bind)).await
}

/// Transitional native feedback remains available while browser parity is audited.
#[cfg(feature = "legacy-native")]
pub async fn run_native(config: VirtualHardwareConfig) -> Result<()> {
    run_with_feedback(config, Feedback::Native).await
}

enum Feedback {
    Headless,
    Browser(SocketAddr),
    #[cfg(feature = "legacy-native")]
    Native,
}

#[derive(Clone)]
struct PlantState {
    endpoint: String,
    config: VirtualHardwareConfig,
    namespace: u16,
    history: Arc<RwLock<VecDeque<TimedSample>>>,
    pending: Arc<RwLock<PendingManualWrites>>,
    node_manager: Arc<SimpleNodeManager>,
}

#[derive(Debug, Deserialize)]
struct ManualValues {
    #[serde(default)]
    external_controller_enable: Option<bool>,
    u_write: f64,
    target: f64,
    #[serde(default)]
    hl: Option<f64>,
    #[serde(default)]
    ll: Option<f64>,
    #[serde(default)]
    band_low: Option<f64>,
    #[serde(default)]
    band_high: Option<f64>,
}

async fn plant_snapshot(State(state): State<PlantState>) -> Json<serde_json::Value> {
    let history = state.history.read();
    let step = history.len().div_ceil(1_200).max(1);
    let samples = history
        .iter()
        .enumerate()
        .filter(|(index, _)| index % step == 0 || *index + 1 == history.len())
        .map(|(_, row)| *row)
        .collect::<Vec<_>>();
    let nodes = [
        Y_VALUE,
        Y_TARGET,
        Y_BAND_LOW,
        Y_BAND_HIGH,
        Y_HL,
        Y_LL,
        U_WRITE,
        U_READ,
        WD_COUNTER,
        WD_MIRROR,
    ]
    .map(|node| format!("ns={};s={node}", state.namespace));
    let address_space = state.node_manager.address_space().read();
    let controller_signals = [CONTROLLER_READY, CONTROLLER_CONTROL, CONTROLLER_ENABLE]
        .into_iter()
        .map(|name| {
            let value = address_space
                .find_node(&NodeId::new(state.namespace, name))
                .and_then(|node| {
                    node.as_node().get_attribute(
                        async_opcua::types::TimestampsToReturn::Neither,
                        AttributeId::Value,
                        &async_opcua::types::NumericRange::None,
                        &DataEncoding::Binary,
                    )
                })
                .and_then(|value| value.value)
                .and_then(|value| {
                    if let async_opcua::types::Variant::Boolean(value) = value {
                        Some(value)
                    } else {
                        None
                    }
                });
            (name, value)
        })
        .collect::<std::collections::HashMap<_, _>>();
    Json(serde_json::json!({
        "controller_signals": controller_signals,
        "endpoint": state.endpoint, "config": state.config, "namespace": state.namespace,
        "history": samples, "latest": history.back(),
        "nodes": nodes,
    }))
}

async fn plant_manual(
    State(state): State<PlantState>,
    Json(values): Json<ManualValues>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    if ![
        Some(values.u_write),
        Some(values.target),
        values.hl,
        values.ll,
        values.band_low,
        values.band_high,
    ]
    .into_iter()
    .flatten()
    .all(|value| value.is_finite())
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "manual values must be finite numbers"})),
        ));
    }
    if matches!((values.band_low, values.band_high), (Some(low), Some(high)) if low > high) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "band low must not exceed band high"})),
        ));
    }
    // One atomic queue update; the existing plant loop writes all requested nodes
    // on its next plant tick, independently of the browser refresh interval.
    *state.pending.write() = PendingManualWrites {
        external_controller_enable: values.external_controller_enable,
        u_write: Some(values.u_write),
        target: Some(values.target),
        hl: values.hl,
        ll: values.ll,
        band_low: values.band_low,
        band_high: values.band_high,
    };
    Ok(Json(serde_json::json!({"queued": true})))
}

async fn run_with_feedback(config: VirtualHardwareConfig, feedback: Feedback) -> Result<()> {
    validate_config(&config)?;

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", config.host, config.port))
        .await
        .with_context(|| {
            format!(
                "failed to bind virtual hardware on {}:{}",
                config.host, config.port
            )
        })?;
    let local_addr = listener
        .local_addr()
        .context("failed to read bound virtual hardware address")?;

    let namespace_uri = "urn:smartapc:virtual-hardware:pt1";
    let pki_dir = virtual_hardware_pki_dir(local_addr.port());
    std::fs::create_dir_all(&pki_dir)
        .with_context(|| format!("failed to create pki dir {}", pki_dir.display()))?;

    let server_host = if local_addr.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        local_addr.ip().to_string()
    };

    let (server, handle) = ServerBuilder::new()
        .application_name("SmartAPC Virtual Hardware PT1")
        .application_uri("urn:smartapc:virtual-hardware:pt1:server")
        .product_uri("urn:smartapc:virtual-hardware:pt1")
        .host(&server_host)
        .port(local_addr.port())
        .create_sample_keypair(true)
        .pki_dir(pki_dir)
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
            "virtual_hardware_pt1",
        ))
        .build()
        .map_err(anyhow::Error::msg)?;

    let node_manager = handle
        .node_managers()
        .get_of_type::<SimpleNodeManager>()
        .ok_or_else(|| anyhow!("virtual hardware node manager not found"))?;

    let ns = handle
        .get_namespace_index(namespace_uri)
        .ok_or_else(|| anyhow!("virtual hardware namespace index not found"))?;

    let folder_id = NodeId::new(ns, "VirtualHardware");
    let y_value_id = NodeId::new(ns, Y_VALUE);
    let y_target_id = NodeId::new(ns, Y_TARGET);
    let y_hl_id = NodeId::new(ns, Y_HL);
    let y_ll_id = NodeId::new(ns, Y_LL);
    let u_write_id = NodeId::new(ns, U_WRITE);
    let u_read_id = NodeId::new(ns, U_READ);
    let wd_counter_id = NodeId::new(ns, WD_COUNTER);
    let wd_mirror_id = NodeId::new(ns, WD_MIRROR);
    {
        let mut address_space = node_manager.address_space().write();
        address_space.add_folder(
            &folder_id,
            "VirtualHardware",
            "VirtualHardware",
            &ObjectId::ObjectsFolder.into(),
        );
        let _ = address_space.add_variables(
            vec![
                rw_bool_var(&NodeId::new(ns, CONTROLLER_READY), CONTROLLER_READY),
                rw_bool_var(&NodeId::new(ns, CONTROLLER_CONTROL), CONTROLLER_CONTROL),
                rw_bool_var(&NodeId::new(ns, CONTROLLER_ENABLE), CONTROLLER_ENABLE),
                ro_f64_var(&y_value_id, Y_VALUE, 0.0),
                rw_f64_var(&y_target_id, Y_TARGET, 0.0),
                rw_f64_var(&y_hl_id, Y_HL, 1.0),
                rw_f64_var(&y_ll_id, Y_LL, -1.0),
                rw_f64_var(&NodeId::new(ns, Y_BAND_LOW), Y_BAND_LOW, -0.5),
                rw_f64_var(&NodeId::new(ns, Y_BAND_HIGH), Y_BAND_HIGH, 0.5),
                rw_f64_var(&u_write_id, U_WRITE, 0.0),
                ro_f64_var(&u_read_id, U_READ, 0.0),
                ro_f64_var(&wd_counter_id, WD_COUNTER, 0.0),
                rw_f64_var(&wd_mirror_id, WD_MIRROR, 0.0),
            ],
            &folder_id,
        );
    }

    let endpoint = format!("opc.tcp://{}:{}/", server_host, local_addr.port());
    let stop = Arc::new(AtomicBool::new(false));
    let history_cap = config
        .history
        .max(min_samples_for_plot_window(config.dt_ms) + 100);
    let history = Arc::new(RwLock::new(VecDeque::<TimedSample>::with_capacity(
        history_cap,
    )));
    let pending_manual_writes = Arc::new(RwLock::new(PendingManualWrites::default()));

    println!("Virtual PT1 hardware started");
    println!("Endpoint: {endpoint}");
    println!("Node addresses:");
    println!("  ns={ns};s={Y_VALUE}");
    println!("  ns={ns};s={Y_TARGET}");
    println!("  ns={ns};s={Y_HL}");
    println!("  ns={ns};s={Y_LL}");
    println!("  ns={ns};s={U_WRITE}");
    println!("  ns={ns};s={U_READ}");
    println!("Additional watchdog test nodes:");
    println!("  ns={ns};s={WD_COUNTER}");
    println!("  ns={ns};s={WD_MIRROR}");
    let plant = PlantState {
        endpoint: endpoint.clone(),
        config: config.clone(),
        namespace: ns,
        history: Arc::clone(&history),
        pending: Arc::clone(&pending_manual_writes),
        node_manager: Arc::clone(&node_manager),
    };

    let server_task = tokio::spawn(async move { server.run_with(listener).await });

    let sim_stop = Arc::clone(&stop);
    let sim_history = Arc::clone(&history);
    let sim_node_manager = Arc::clone(&node_manager);
    let subscriptions = Arc::clone(handle.subscriptions());
    let sim_pending_manual_writes = Arc::clone(&pending_manual_writes);
    let sim_task = tokio::spawn(async move {
        let mut state = Sample {
            y: 0.0,
            u: 0.0,
            target: 0.0,
            hl: 1.0,
            ll: -1.0,
            band_low: -0.5,
            band_high: 0.5,
        };
        let mut wd_counter = 0.0f64;
        let mut tick = tokio::time::interval(Duration::from_millis(config.dt_ms));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let start = Instant::now();

        loop {
            tick.tick().await;
            if sim_stop.load(Ordering::SeqCst) {
                break;
            }

            let pending = take_pending_manual_writes(&sim_pending_manual_writes);
            if let Some(value) = pending.external_controller_enable {
                let _ = sim_node_manager.set_value(
                    subscriptions.as_ref(),
                    &NodeId::new(ns, CONTROLLER_ENABLE),
                    None,
                    DataValue::new_now(value),
                );
            }
            if let Some(value) = pending.u_write {
                let _ = sim_node_manager.set_value(
                    subscriptions.as_ref(),
                    &u_write_id,
                    None,
                    DataValue::new_now(value),
                );
            }
            if let Some(value) = pending.target {
                let _ = sim_node_manager.set_value(
                    subscriptions.as_ref(),
                    &y_target_id,
                    None,
                    DataValue::new_now(value),
                );
            }
            if let Some(value) = pending.hl {
                let _ = sim_node_manager.set_value(
                    subscriptions.as_ref(),
                    &y_hl_id,
                    None,
                    DataValue::new_now(value),
                );
            }
            if let Some(value) = pending.ll {
                let _ = sim_node_manager.set_value(
                    subscriptions.as_ref(),
                    &y_ll_id,
                    None,
                    DataValue::new_now(value),
                );
            }

            for (node, value) in [
                (Y_BAND_LOW, pending.band_low),
                (Y_BAND_HIGH, pending.band_high),
            ] {
                if let Some(value) = value {
                    let _ = sim_node_manager.set_value(
                        subscriptions.as_ref(),
                        &NodeId::new(ns, node),
                        None,
                        DataValue::new_now(value),
                    );
                }
            }
            {
                let address_space = sim_node_manager.address_space().read();
                state.band_low = read_node_f64(&address_space, &NodeId::new(ns, Y_BAND_LOW))
                    .unwrap_or(state.band_low);
                state.band_high = read_node_f64(&address_space, &NodeId::new(ns, Y_BAND_HIGH))
                    .unwrap_or(state.band_high);
            }
            let external = read_external_inputs_from_nodes(
                &sim_node_manager,
                &y_target_id,
                &y_hl_id,
                &y_ll_id,
                &u_write_id,
                ExternalInputs {
                    u_write: state.u,
                    target: state.target,
                    hl: state.hl,
                    ll: state.ll,
                },
            );
            state.u = external.u_write;
            state.target = external.target;
            state.hl = external.hl;
            state.ll = external.ll;

            let dt_s = config.dt_ms as f64 / 1_000.0;
            state.y += (dt_s / config.tau_s) * ((config.gain * state.u) - state.y);

            let _ = sim_node_manager.set_value(
                subscriptions.as_ref(),
                &y_value_id,
                None,
                DataValue::new_now(state.y),
            );
            let _ = sim_node_manager.set_value(
                subscriptions.as_ref(),
                &u_read_id,
                None,
                DataValue::new_now(state.u),
            );
            wd_counter += 1.0;
            let _ = sim_node_manager.set_value(
                subscriptions.as_ref(),
                &wd_counter_id,
                None,
                DataValue::new_now(wd_counter),
            );

            let now_s = start.elapsed().as_secs_f64();
            let mut h = sim_history.write();
            h.push_back(TimedSample {
                t_s: now_s,
                sample: state,
            });
            while h.len() > history_cap {
                let _ = h.pop_front();
            }
        }
    });

    let signal_stop = Arc::clone(&stop);
    let signal_task = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_stop.store(true, Ordering::SeqCst);
    });

    let plot_result = match feedback {
        Feedback::Headless => {
            println!("Headless plant running; Ctrl+C stops the OPC UA server.");
            while !stop.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok(())
        }
        #[cfg(feature = "legacy-native")]
        Feedback::Native => run_plot_window(
            endpoint.clone(),
            Arc::clone(&history),
            Arc::clone(&stop),
            Arc::clone(&pending_manual_writes),
        ),
        Feedback::Browser(bind) => {
            // Keep listener errors inside this result so plant cleanup always runs.
            async {
                let assets = crate::web_assets::FrontendAssets::locate(&std::env::current_exe()?);
                let app = Router::new()
                    .route(
                        "/",
                        get(|| async { Redirect::temporary("/virtual-hardware") }),
                    )
                    .route(
                        "/api/virtual-hardware",
                        get(plant_snapshot).post(plant_manual),
                    )
                    .fallback(crate::web_assets::spa)
                    .layer(axum::Extension(assets))
                    .with_state(plant);
                let listener = tokio::net::TcpListener::bind(bind)
                    .await
                    .context("failed to bind virtual hardware browser view")?;
                println!(
                    "Browser feedback: http://{}/virtual-hardware",
                    listener.local_addr()?
                );
                let browser_stop = Arc::clone(&stop);
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        while !browser_stop.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    })
                    .await
                    .context("virtual hardware browser server failed")
            }
            .await
        }
    };
    stop.store(true, Ordering::SeqCst);
    signal_task.abort();

    sim_task
        .await
        .context("virtual hardware simulation task failed")?;

    handle.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .context("timed out waiting for virtual hardware server shutdown")??;

    println!("Virtual PT1 hardware stopped.");
    plot_result
}

fn validate_config(config: &VirtualHardwareConfig) -> Result<()> {
    if config.dt_ms == 0 {
        return Err(anyhow!("dt-ms must be > 0"));
    }
    if !(config.tau_s.is_finite() && config.tau_s > 0.0) {
        return Err(anyhow!("tau-s must be finite and > 0"));
    }
    if !config.gain.is_finite() {
        return Err(anyhow!("gain must be finite"));
    }
    if config.history == 0 {
        return Err(anyhow!("history must be > 0"));
    }
    Ok(())
}

fn min_samples_for_plot_window(dt_ms: u64) -> usize {
    ((PLOT_WINDOW_SECONDS * 1_000.0) / dt_ms as f64).ceil() as usize
}

fn virtual_hardware_pki_dir(port: u16) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    std::env::temp_dir().join(format!("smartapc_virtual_hw_pki_{port}_{now}"))
}

fn rw_bool_var(id: &NodeId, browse_name: &str) -> Variable {
    let mut var = Variable::new(id, browse_name, browse_name, false);
    var.set_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
    var.set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
    var
}

fn rw_f64_var(id: &NodeId, browse_name: &str, initial: f64) -> Variable {
    let mut var = Variable::new(id, browse_name, browse_name, initial);
    var.set_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
    var.set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
    var
}

fn ro_f64_var(id: &NodeId, browse_name: &str, initial: f64) -> Variable {
    let mut var = Variable::new(id, browse_name, browse_name, initial);
    var.set_access_level(AccessLevel::CURRENT_READ);
    var.set_user_access_level(AccessLevel::CURRENT_READ);
    var
}

fn read_external_inputs_from_nodes(
    node_manager: &SimpleNodeManager,
    y_target_id: &NodeId,
    y_hl_id: &NodeId,
    y_ll_id: &NodeId,
    u_write_id: &NodeId,
    fallback: ExternalInputs,
) -> ExternalInputs {
    let address_space = node_manager.address_space().read();
    ExternalInputs {
        u_write: read_node_f64(&address_space, u_write_id).unwrap_or(fallback.u_write),
        target: read_node_f64(&address_space, y_target_id).unwrap_or(fallback.target),
        hl: read_node_f64(&address_space, y_hl_id).unwrap_or(fallback.hl),
        ll: read_node_f64(&address_space, y_ll_id).unwrap_or(fallback.ll),
    }
}

fn read_node_f64(
    address_space: &async_opcua::server::address_space::AddressSpace,
    node_id: &NodeId,
) -> Option<f64> {
    let node = address_space.find_node(node_id)?;
    let data_value = node.as_node().get_attribute(
        async_opcua::types::TimestampsToReturn::Neither,
        AttributeId::Value,
        &async_opcua::types::NumericRange::None,
        &DataEncoding::Binary,
    )?;
    data_value.value.and_then(|v| v.as_f64())
}

fn take_pending_manual_writes(
    pending_manual_writes: &RwLock<PendingManualWrites>,
) -> PendingManualWrites {
    let mut guard = pending_manual_writes.write();
    let current = *guard;
    *guard = PendingManualWrites::default();
    current
}

#[cfg(any(feature = "legacy-native", test))]
fn parse_manual_input(raw: &str, variable_name: &str) -> Result<f64, String> {
    raw.trim()
        .parse::<f64>()
        .map_err(|_| format!("{variable_name} must be a valid floating-point number"))
}

#[derive(Clone)]
#[cfg(feature = "legacy-native")]
struct PlotWindowApp {
    endpoint: String,
    history: Arc<RwLock<VecDeque<TimedSample>>>,
    stop: Arc<AtomicBool>,
    pending_manual_writes: Arc<RwLock<PendingManualWrites>>,
    u_write_input: String,
    target_input: String,
    hl_input: String,
    ll_input: String,
    feedback: Option<(bool, String)>,
}

#[cfg(feature = "legacy-native")]
impl PlotWindowApp {
    fn new(
        endpoint: String,
        history: Arc<RwLock<VecDeque<TimedSample>>>,
        stop: Arc<AtomicBool>,
        pending_manual_writes: Arc<RwLock<PendingManualWrites>>,
    ) -> Self {
        Self {
            endpoint,
            history,
            stop,
            pending_manual_writes,
            u_write_input: "0.0".to_string(),
            target_input: "0.0".to_string(),
            hl_input: "0.5".to_string(),
            ll_input: "-0.5".to_string(),
            feedback: None,
        }
    }

    fn apply_manual_inputs(&mut self) {
        let u_write = match parse_manual_input(&self.u_write_input, U_WRITE) {
            Ok(value) => value,
            Err(error) => {
                self.feedback = Some((true, error));
                return;
            }
        };
        let target = match parse_manual_input(&self.target_input, Y_TARGET) {
            Ok(value) => value,
            Err(error) => {
                self.feedback = Some((true, error));
                return;
            }
        };
        let hl = match parse_manual_input(&self.hl_input, Y_BAND_HIGH) {
            Ok(value) => value,
            Err(error) => {
                self.feedback = Some((true, error));
                return;
            }
        };
        let ll = match parse_manual_input(&self.ll_input, Y_BAND_LOW) {
            Ok(value) => value,
            Err(error) => {
                self.feedback = Some((true, error));
                return;
            }
        };

        *self.pending_manual_writes.write() = PendingManualWrites {
            external_controller_enable: None,
            u_write: Some(u_write),
            target: Some(target),
            band_high: Some(hl),
            band_low: Some(ll),
            ..PendingManualWrites::default()
        };
        self.feedback = Some((
            false,
            "Manual input values queued for the next tick.".to_string(),
        ));
    }
}

#[cfg(feature = "legacy-native")]
impl eframe::App for PlotWindowApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.stop.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        ctx.request_repaint_after(Duration::from_millis(PLOT_REFRESH_MS));
        let snapshot: Vec<TimedSample> = self.history.read().iter().copied().collect();
        if snapshot.is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("SmartAPC Virtual Hardware");
                ui.label(format!("Endpoint: {}", self.endpoint));
                ui.label("Waiting for simulation samples...");
            });
            return;
        }

        let newest_t = snapshot.last().map(|s| s.t_s).unwrap_or(0.0);
        let min_t = (newest_t - PLOT_WINDOW_SECONDS).max(0.0);
        let max_t = min_t + PLOT_WINDOW_SECONDS;

        let visible: Vec<TimedSample> = snapshot.into_iter().filter(|s| s.t_s >= min_t).collect();
        let current = visible.last().map(|s| s.sample).unwrap_or(Sample {
            y: 0.0,
            u: 0.0,
            target: 0.0,
            hl: 1.0,
            ll: -1.0,
            band_low: -0.5,
            band_high: 0.5,
        });

        let y_points: PlotPoints<'_> = visible.iter().map(|s| [s.t_s, s.sample.y]).collect();
        let target_points: PlotPoints<'_> =
            visible.iter().map(|s| [s.t_s, s.sample.target]).collect();
        let hl_points: PlotPoints<'_> = visible
            .iter()
            .map(|s| [s.t_s, s.sample.target + s.sample.band_high])
            .collect();
        let ll_points: PlotPoints<'_> = visible
            .iter()
            .map(|s| [s.t_s, s.sample.target + s.sample.band_low])
            .collect();
        let u_points: PlotPoints<'_> = visible.iter().map(|s| [s.t_s, s.sample.u]).collect();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("SmartAPC Virtual Hardware PT1");
            ui.label(format!("Endpoint: {}", self.endpoint));
            ui.label(format!(
                "Current values | y1_value={:.4}  u1={:.4}  y1_target={:.4}  band_high={:.4}  band_low={:.4}",
                current.y, current.u, current.target, current.band_high, current.band_low
            ));
            ui.separator();
            ui.label("Manual setpoints (text input):");
            ui.horizontal(|ui| {
                ui.label(format!("{U_WRITE}:"));
                ui.text_edit_singleline(&mut self.u_write_input);
                ui.label(format!("{Y_TARGET}:"));
                ui.text_edit_singleline(&mut self.target_input);
            });
            ui.horizontal(|ui| {
                ui.label(format!("{Y_BAND_HIGH}:"));
                ui.text_edit_singleline(&mut self.hl_input);
                ui.label(format!("{Y_BAND_LOW}:"));
                ui.text_edit_singleline(&mut self.ll_input);
            });
            if ui.button("Apply Manual Values").clicked() {
                self.apply_manual_inputs();
            }
            if let Some((is_error, message)) = &self.feedback {
                let color = if *is_error {
                    egui::Color32::from_rgb(220, 90, 90)
                } else {
                    egui::Color32::from_rgb(100, 180, 100)
                };
                ui.colored_label(color, message);
            }
            ui.separator();

            Plot::new("pt1_y_plot")
                .height(260.0)
                .legend(Legend::default())
                .allow_scroll(false)
                .allow_drag(false)
                .allow_zoom(false)
                .include_x(min_t)
                .include_x(max_t)
                .show(ui, |plot_ui| {
                    plot_ui.line(
                        Line::new("y1_value", y_points)
                            .color(egui::Color32::from_rgb(50, 180, 220)),
                    );
                    plot_ui.line(
                        Line::new("y1_target", target_points)
                            .color(egui::Color32::from_rgb(255, 200, 0)),
                    );
                    plot_ui.line(
                        Line::new("Band high", hl_points).color(egui::Color32::from_rgb(220, 90, 90)),
                    );
                    plot_ui.line(
                        Line::new("Band low", ll_points).color(egui::Color32::from_rgb(220, 90, 90)),
                    );
                });

            Plot::new("pt1_u_plot")
                .height(220.0)
                .legend(Legend::default())
                .allow_scroll(false)
                .allow_drag(false)
                .allow_zoom(false)
                .include_x(min_t)
                .include_x(max_t)
                .show(ui, |plot_ui| {
                    plot_ui
                        .line(Line::new("u1_write/u1_read", u_points).color(egui::Color32::GREEN));
                });
        });
    }
}

#[cfg(feature = "legacy-native")]
fn run_plot_window(
    endpoint: String,
    history: Arc<RwLock<VecDeque<TimedSample>>>,
    stop: Arc<AtomicBool>,
    pending_manual_writes: Arc<RwLock<PendingManualWrites>>,
) -> Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 760.0]),
        ..Default::default()
    };

    eframe::run_native(
        "SmartAPC Virtual Hardware Plot",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(PlotWindowApp::new(
                endpoint.clone(),
                Arc::clone(&history),
                Arc::clone(&stop),
                Arc::clone(&pending_manual_writes),
            )))
        }),
    )
    .map_err(|err| anyhow!("failed to launch plot window: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_opcua::server::address_space::{AddressSpace, NodeType};
    use async_opcua::types::{NumericRange, ObjectId};

    #[test]
    fn read_node_f64_returns_current_written_value() {
        let mut address_space = AddressSpace::new();
        let ns = 2u16;
        address_space.add_namespace("urn:smartapc:test", ns);

        let node_id = NodeId::new(ns, "u1_write");
        let inserted = address_space.add_variables(
            vec![rw_f64_var(&node_id, "u1_write", 0.0)],
            &ObjectId::ObjectsFolder.into(),
        );
        assert_eq!(inserted, vec![true]);

        let node = address_space
            .find_node_mut(&node_id)
            .expect("u1_write node present");
        match node {
            NodeType::Variable(var) => {
                var.set_value(&NumericRange::None, 1.0f64)
                    .expect("set variable value");
            }
            _ => panic!("u1_write must be a variable"),
        }

        let value = read_node_f64(&address_space, &node_id).expect("read f64 value");
        assert_eq!(value, 1.0);
    }

    #[test]
    fn parse_manual_input_rejects_invalid_numbers() {
        let error = parse_manual_input("not-a-number", U_WRITE).expect_err("must fail");
        assert_eq!(
            error,
            "u1_write must be a valid floating-point number".to_string()
        );
    }

    #[test]
    fn take_pending_manual_writes_returns_and_clears_values() {
        let pending = RwLock::new(PendingManualWrites {
            external_controller_enable: None,
            u_write: Some(1.25),
            target: Some(2.5),
            hl: Some(3.75),
            ll: Some(-0.5),
            band_low: Some(-0.5),
            band_high: Some(0.5),
        });

        let first = take_pending_manual_writes(&pending);
        assert_eq!(first.u_write, Some(1.25));
        assert_eq!(first.target, Some(2.5));
        assert_eq!(first.hl, Some(3.75));
        assert_eq!(first.ll, Some(-0.5));
        assert_eq!(first.band_low, Some(-0.5));
        assert_eq!(first.band_high, Some(0.5));

        let second = take_pending_manual_writes(&pending);
        assert_eq!(second.u_write, None);
        assert_eq!(second.target, None);
        assert_eq!(second.hl, None);
        assert_eq!(second.ll, None);
        assert_eq!(second.band_low, None);
        assert_eq!(second.band_high, None);
    }
}
