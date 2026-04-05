use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
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
    types::{DataValue, MessageSecurityMode, NodeId, ObjectId, StatusCode},
};
use parking_lot::RwLock;

const Y_SP: &str = "y1_sp";
const Y_TARGET: &str = "y1_target";
const Y_HL: &str = "y1_HL";
const Y_LL: &str = "y1_LL";
const U_WRITE: &str = "u1_write";
const U_READ: &str = "u1_read";
const WD_COUNTER: &str = "wd_counter_read";
const WD_MIRROR: &str = "wd_counter_mirror";

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
    let y_sp_id = NodeId::new(ns, Y_SP);
    let y_target_id = NodeId::new(ns, Y_TARGET);
    let y_hl_id = NodeId::new(ns, Y_HL);
    let y_ll_id = NodeId::new(ns, Y_LL);
    let u_write_id = NodeId::new(ns, U_WRITE);
    let u_read_id = NodeId::new(ns, U_READ);
    let wd_counter_id = NodeId::new(ns, WD_COUNTER);
    let wd_mirror_id = NodeId::new(ns, WD_MIRROR);
    let external_inputs = Arc::new(RwLock::new(ExternalInputs::default()));

    add_external_input_callback(
        &node_manager,
        y_target_id.clone(),
        Arc::clone(&external_inputs),
        |v, x| v.target = x,
    );
    add_external_input_callback(
        &node_manager,
        y_hl_id.clone(),
        Arc::clone(&external_inputs),
        |v, x| v.hl = x,
    );
    add_external_input_callback(
        &node_manager,
        y_ll_id.clone(),
        Arc::clone(&external_inputs),
        |v, x| v.ll = x,
    );
    add_external_input_callback(
        &node_manager,
        u_write_id.clone(),
        Arc::clone(&external_inputs),
        |v, x| v.u_write = x,
    );

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
                rw_f64_var(&y_sp_id, Y_SP, 0.0),
                rw_f64_var(&y_target_id, Y_TARGET, 0.0),
                rw_f64_var(&y_hl_id, Y_HL, 1.0),
                rw_f64_var(&y_ll_id, Y_LL, -1.0),
                rw_f64_var(&u_write_id, U_WRITE, 0.0),
                rw_f64_var(&u_read_id, U_READ, 0.0),
                rw_f64_var(&wd_counter_id, WD_COUNTER, 0.0),
                rw_f64_var(&wd_mirror_id, WD_MIRROR, 0.0),
            ],
            &folder_id,
        );
    }

    let server_task = tokio::spawn(async move { server.run_with(listener).await });
    let endpoint = format!("opc.tcp://{}:{}/", server_host, local_addr.port());

    println!("Virtual PT1 hardware started");
    println!("Endpoint: {endpoint}");
    println!("Node addresses:");
    println!("  ns={ns};s={Y_SP}");
    println!("  ns={ns};s={Y_TARGET}");
    println!("  ns={ns};s={Y_HL}");
    println!("  ns={ns};s={Y_LL}");
    println!("  ns={ns};s={U_WRITE}");
    println!("  ns={ns};s={U_READ}");
    println!("Additional watchdog test nodes:");
    println!("  ns={ns};s={WD_COUNTER}");
    println!("  ns={ns};s={WD_MIRROR}");
    println!("Press Ctrl+C to stop.");

    let mut state = Sample {
        y: 0.0,
        u: 0.0,
        target: 0.0,
        hl: 1.0,
        ll: -1.0,
    };
    let mut wd_counter = 0.0f64;
    let subscriptions = Arc::clone(handle.subscriptions());

    let mut history = VecDeque::with_capacity(config.history.max(20));
    let mut tick = tokio::time::interval(Duration::from_millis(config.dt_ms));
    let mut draw_tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    draw_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                break;
            }
            _ = tick.tick() => {
                let external = *external_inputs.read();
                state.u = external.u_write;
                state.target = external.target;
                state.hl = external.hl;
                state.ll = external.ll;

                let dt_s = config.dt_ms as f64 / 1_000.0;
                state.y += (dt_s / config.tau_s) * ((config.gain * state.u) - state.y);

                let _ = node_manager.set_value(
                    subscriptions.as_ref(),
                    &y_sp_id,
                    None,
                    DataValue::new_now(state.y),
                );
                let _ = node_manager.set_value(
                    subscriptions.as_ref(),
                    &u_read_id,
                    None,
                    DataValue::new_now(state.u),
                );
                wd_counter += 1.0;
                let _ = node_manager.set_value(
                    subscriptions.as_ref(),
                    &wd_counter_id,
                    None,
                    DataValue::new_now(wd_counter),
                );

                history.push_back(state);
                while history.len() > config.history {
                    let _ = history.pop_front();
                }
            }
            _ = draw_tick.tick() => {
                render_sliding_plot(&history, config.history, state, &endpoint);
            }
        }
    }

    handle.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .context("timed out waiting for virtual hardware server shutdown")??;

    println!("Virtual PT1 hardware stopped.");
    Ok(())
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
    if config.history < 20 {
        return Err(anyhow!("history must be >= 20"));
    }
    Ok(())
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

fn add_external_input_callback(
    node_manager: &SimpleNodeManager,
    node_id: NodeId,
    state: Arc<RwLock<ExternalInputs>>,
    setter: fn(&mut ExternalInputs, f64),
) {
    node_manager
        .inner()
        .add_write_callback(node_id, move |value, _index_range| {
            let Some(next_value) = value.value.and_then(|v| v.as_f64()) else {
                return StatusCode::BadTypeMismatch;
            };
            let mut guard = state.write();
            setter(&mut guard, next_value);
            StatusCode::Good
        });
}

fn render_sliding_plot(history: &VecDeque<Sample>, width: usize, current: Sample, endpoint: &str) {
    let width = width.clamp(40, 200);
    let series: Vec<_> = history.iter().copied().collect();
    if series.is_empty() {
        return;
    }

    let y_values = series.iter().map(|s| s.y).collect::<Vec<_>>();
    let target_values = series.iter().map(|s| s.target).collect::<Vec<_>>();
    let hl_values = series.iter().map(|s| s.hl).collect::<Vec<_>>();
    let ll_values = series.iter().map(|s| s.ll).collect::<Vec<_>>();
    let u_values = series.iter().map(|s| s.u).collect::<Vec<_>>();

    let y_min = y_values
        .iter()
        .chain(target_values.iter())
        .chain(hl_values.iter())
        .chain(ll_values.iter())
        .copied()
        .fold(f64::INFINITY, f64::min);
    let y_max = y_values
        .iter()
        .chain(target_values.iter())
        .chain(hl_values.iter())
        .chain(ll_values.iter())
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    let u_min = u_values
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min)
        .min(0.0);
    let u_max = u_values
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
        .max(0.0);

    let y_chart = ascii_chart(
        width,
        12,
        y_min,
        y_max,
        &[
            (&hl_values, '-'),
            (&ll_values, '-'),
            (&target_values, '='),
            (&y_values, '*'),
        ],
    );
    let u_chart = ascii_chart(width, 8, u_min, u_max, &[(&u_values, '#')]);

    print!("\x1B[2J\x1B[H");
    println!(
        "SmartAPC Virtual PT1 | endpoint={endpoint} | y={:.4} u={:.4} target={:.4} HL={:.4} LL={:.4}",
        current.y, current.u, current.target, current.hl, current.ll
    );
    println!("y chart legend: '*'=y1_sp '='=y1_target '-'=y1_HL/y1_LL");
    for line in y_chart {
        println!("{line}");
    }
    println!("u chart legend: '#'=u1 (u1_write/u1_read)");
    for line in u_chart {
        println!("{line}");
    }
}

fn ascii_chart(
    width: usize,
    height: usize,
    min_v: f64,
    max_v: f64,
    series: &[(&[f64], char)],
) -> Vec<String> {
    let mut canvas = vec![vec![' '; width]; height];
    let span = (max_v - min_v).abs();

    for (values, marker) in series {
        let sampled = resample_tail(values, width);
        for (x, value) in sampled.into_iter().enumerate() {
            let row = if span < f64::EPSILON {
                height / 2
            } else {
                let norm = ((value - min_v) / (max_v - min_v)).clamp(0.0, 1.0);
                let from_bottom = (norm * (height as f64 - 1.0)).round() as usize;
                (height - 1).saturating_sub(from_bottom)
            };
            if row < height && x < width {
                canvas[row][x] = *marker;
            }
        }
    }

    let mut lines = Vec::with_capacity(height + 2);
    lines.push(format!("max={max_v:.4}"));
    for row in canvas {
        lines.push(row.into_iter().collect());
    }
    lines.push(format!("min={min_v:.4}"));
    lines
}

fn resample_tail(values: &[f64], width: usize) -> Vec<f64> {
    if values.is_empty() || width == 0 {
        return Vec::new();
    }
    if values.len() <= width {
        return values.to_vec();
    }
    values[values.len() - width..].to_vec()
}
