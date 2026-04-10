use std::{
    collections::VecDeque,
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
use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use parking_lot::RwLock;

const Y_VALUE: &str = "y1_value";
const Y_TARGET: &str = "y1_target";
const Y_HL: &str = "y1_HL";
const Y_LL: &str = "y1_LL";
const U_WRITE: &str = "u1_write";
const U_READ: &str = "u1_read";
const WD_COUNTER: &str = "wd_counter_read";
const WD_MIRROR: &str = "wd_counter_mirror";

const PLOT_WINDOW_SECONDS: f64 = 180.0;
const PLOT_REFRESH_MS: u64 = 100;

#[derive(Debug, Clone)]
pub struct VirtualHardwareConfig {
    pub host: String,
    pub port: u16,
    pub dt_ms: u64,
    pub tau_s: f64,
    pub gain: f64,
    pub history: usize,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    y: f64,
    u: f64,
    target: f64,
    hl: f64,
    ll: f64,
}

#[derive(Debug, Clone, Copy)]
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

pub async fn run(config: VirtualHardwareConfig) -> Result<()> {
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
                ro_f64_var(&y_value_id, Y_VALUE, 0.0),
                rw_f64_var(&y_target_id, Y_TARGET, 0.0),
                rw_f64_var(&y_hl_id, Y_HL, 1.0),
                rw_f64_var(&y_ll_id, Y_LL, -1.0),
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
    println!("Opening live plot window with fixed 3 minute time range...");

    let server_task = tokio::spawn(async move { server.run_with(listener).await });

    let sim_stop = Arc::clone(&stop);
    let sim_history = Arc::clone(&history);
    let sim_node_manager = Arc::clone(&node_manager);
    let subscriptions = Arc::clone(handle.subscriptions());
    let sim_task = tokio::spawn(async move {
        let mut state = Sample {
            y: 0.0,
            u: 0.0,
            target: 0.0,
            hl: 1.0,
            ll: -1.0,
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

    let plot_result = run_plot_window(endpoint.clone(), Arc::clone(&history), Arc::clone(&stop));
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

#[derive(Clone)]
struct PlotWindowApp {
    endpoint: String,
    history: Arc<RwLock<VecDeque<TimedSample>>>,
    stop: Arc<AtomicBool>,
}

impl PlotWindowApp {
    fn new(
        endpoint: String,
        history: Arc<RwLock<VecDeque<TimedSample>>>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            endpoint,
            history,
            stop,
        }
    }
}

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
        });

        let y_points: PlotPoints<'_> = visible.iter().map(|s| [s.t_s, s.sample.y]).collect();
        let target_points: PlotPoints<'_> =
            visible.iter().map(|s| [s.t_s, s.sample.target]).collect();
        let hl_points: PlotPoints<'_> = visible.iter().map(|s| [s.t_s, s.sample.hl]).collect();
        let ll_points: PlotPoints<'_> = visible.iter().map(|s| [s.t_s, s.sample.ll]).collect();
        let u_points: PlotPoints<'_> = visible.iter().map(|s| [s.t_s, s.sample.u]).collect();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("SmartAPC Virtual Hardware PT1");
            ui.label(format!("Endpoint: {}", self.endpoint));
            ui.label(format!(
                "Current values | y1_value={:.4}  u1={:.4}  y1_target={:.4}  y1_HL={:.4}  y1_LL={:.4}",
                current.y, current.u, current.target, current.hl, current.ll
            ));

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
                        Line::new("y1_HL", hl_points).color(egui::Color32::from_rgb(220, 90, 90)),
                    );
                    plot_ui.line(
                        Line::new("y1_LL", ll_points).color(egui::Color32::from_rgb(220, 90, 90)),
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

fn run_plot_window(
    endpoint: String,
    history: Arc<RwLock<VecDeque<TimedSample>>>,
    stop: Arc<AtomicBool>,
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
}
