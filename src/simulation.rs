use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::json;

use crate::{
    logic::{ControlLogic, LogicUserLog},
    project_config::{
        LimitsConfig, LinearStateSpaceModelConfig, LogicParamValue, ProjectConfig,
        SimulationConfig, SimulationSpeedMode,
    },
    runtime::RuntimeMetrics,
    storage::{SeriesCatalogEntry, SignalSample, SqliteStorage},
    types::{RuntimeState, SignalValue},
};

#[derive(Debug, Clone, Serialize)]
pub struct SimulationSnapshot {
    pub project_name: String,
    pub state: RuntimeState,
    pub metrics: RuntimeMetrics,
    pub last_inputs: HashMap<String, SignalValue>,
    pub last_outputs: HashMap<String, SignalValue>,
    pub simulation_time_s: f64,
}

pub(crate) struct SimulationCore {
    config: ProjectConfig,
    logic_io: crate::project_config::IoConfig,
    limits: LimitsConfig,
    model: LinearStateSpaceModelConfig,
    simulation: SimulationConfig,
    logic: Box<dyn ControlLogic>,
    storage: SqliteStorage,
    log_file: File,
    rng: SmallRng,
    state: RuntimeState,
    plant_state: Vec<f64>,
    current_cv: HashMap<String, f64>,
    current_mv: HashMap<String, f64>,
    current_dv: HashMap<String, f64>,
    applied_mv: HashMap<String, f64>,
    last_inputs: HashMap<String, SignalValue>,
    last_outputs: HashMap<String, SignalValue>,
    total_cycles: u64,
    deadline_misses: u64,
    last_duration_us: u64,
    last_jitter_us: i64,
    last_cycle_start: Option<Instant>,
    simulation_time_s: f64,
}

impl SimulationCore {
    pub(crate) fn new(
        mut config: ProjectConfig,
        logic_io: crate::project_config::IoConfig,
        limits: LimitsConfig,
        mut logic: Box<dyn ControlLogic>,
        storage: SqliteStorage,
        log_file: File,
    ) -> Result<Self> {
        config.normalize_simulation_config();
        config.sync_active_online_params();
        config.validate_for_simulation()?;
        logic.init(
            config.logic.initial_values.clone(),
            config.logic.params.clone(),
        );

        let model = config
            .model
            .as_ref()
            .and_then(|model| model.linear_state_space())
            .cloned()
            .ok_or_else(|| anyhow!("simulation requires a linear state-space model"))?;

        let mut core = Self {
            current_cv: config.simulation.cv_initials.clone(),
            current_mv: config.simulation.mv_initials.clone(),
            current_dv: config.simulation.dv_initials.clone(),
            applied_mv: config.simulation.mv_initials.clone(),
            plant_state: vec![0.0; model.state_count],
            last_inputs: HashMap::new(),
            last_outputs: HashMap::new(),
            total_cycles: 0,
            deadline_misses: 0,
            last_duration_us: 0,
            last_jitter_us: 0,
            last_cycle_start: None,
            simulation_time_s: 0.0,
            rng: SmallRng::seeded(),
            state: RuntimeState::Running,
            simulation: config.simulation.clone(),
            model,
            config,
            logic_io,
            limits,
            logic,
            storage,
            log_file,
        };
        core.initialize_steady_state()?;
        core.last_inputs = core.build_inputs();
        Ok(core)
    }

    pub(crate) fn snapshot(&self) -> SimulationSnapshot {
        SimulationSnapshot {
            project_name: self.config.name.clone(),
            state: self.state,
            metrics: RuntimeMetrics {
                total_cycles: self.total_cycles,
                deadline_misses: self.deadline_misses,
                last_duration_us: self.last_duration_us,
                last_jitter_us: self.last_jitter_us,
            },
            last_inputs: self.last_inputs.clone(),
            last_outputs: self.last_outputs.clone(),
            simulation_time_s: self.simulation_time_s,
        }
    }

    pub(crate) fn current_params(&self) -> HashMap<String, LogicParamValue> {
        self.logic.current_params()
    }

    pub(crate) fn set_params(&mut self, updates: HashMap<String, LogicParamValue>) -> Result<()> {
        let current = self.logic.current_params();
        for (key, value) in &updates {
            let existing = current
                .get(key)
                .ok_or_else(|| anyhow!("unknown online parameter '{}'", key))?;
            if !existing.matches_type(value.param_type()) {
                bail!(
                    "online parameter '{}' type mismatch (expected {:?})",
                    key,
                    existing.param_type()
                );
            }
        }

        self.logic.set_params(updates.clone());
        self.append_log_record(json!({
            "timestamp_ms": now_ms(),
            "event": "simulation_online_params_update",
            "state": self.state,
            "updates": updates,
            "params": self.logic.current_params(),
            "simulation_time_s": self.simulation_time_s,
        }))?;
        Ok(())
    }

    pub(crate) fn apply_live_config(&mut self, config: &SimulationConfig) {
        self.simulation.cv_setpoints = config.cv_setpoints.clone();
        self.simulation.cv_noise_std = config.cv_noise_std.clone();
        self.simulation.mv_noise_std = config.mv_noise_std.clone();
        self.simulation.dv_disturbances = config.dv_disturbances.clone();
        self.simulation.cv_offsets = config.cv_offsets.clone();
        self.simulation.a_scale_pct = config.a_scale_pct.clone();
        self.simulation.b_scale_pct = config.b_scale_pct.clone();
    }

    pub(crate) fn read_plot_samples(&self, limit: usize) -> Result<Vec<SignalSample>> {
        self.storage.read_recent_samples(limit)
    }

    pub(crate) fn read_plot_samples_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        let since = now_ms().saturating_sub(window_ms as i64);
        self.storage.read_samples_since(since)
    }

    pub(crate) fn read_plot_catalog(&self) -> Result<Vec<SeriesCatalogEntry>> {
        self.storage.read_series_catalog()
    }

    pub(crate) fn tick_once(&mut self, target_cycle: Option<Duration>) -> Result<()> {
        let cycle_start = Instant::now();
        let dt_s = self.config.runtime.cycle_ms as f64 / 1000.0;

        let inputs = self.build_inputs();
        self.storage.store_signal_map("input", &inputs)?;
        self.last_inputs = inputs.clone();

        let mut outputs = self.logic.step(&inputs, RuntimeState::Running)?;
        let user_logs = self.logic.drain_user_logs();
        apply_output_limits(&self.limits, &mut outputs);

        let mut applied_mv = self.applied_mv.clone();
        for name in &self.config.variables.mvs {
            let signal = format!("mv.{}.write", name);
            if let Some(value) = outputs.get(&signal).and_then(SignalValue::as_f64) {
                applied_mv.insert(name.clone(), value);
            }
        }

        self.last_outputs = outputs.clone();
        self.storage.store_signal_map("output", &outputs)?;
        self.store_online_param_samples()?;
        self.store_user_log_samples(&user_logs)?;
        self.append_cycle_log(&inputs, &outputs, &user_logs)?;

        let (a_scaled, b_scaled) = self.scaled_model_matrices();
        let u = self.current_u_vector(&applied_mv, &self.current_dv);
        self.plant_state = rk4_step(&a_scaled, &b_scaled, &self.plant_state, &u, dt_s);
        self.applied_mv = applied_mv.clone();
        self.simulation_time_s += dt_s;

        let y_model = output_vector(&self.model, &self.plant_state, &u);
        self.update_measurements_from_output(&applied_mv, &y_model);

        let duration = cycle_start.elapsed();
        self.update_metrics(cycle_start, duration, target_cycle);
        self.total_cycles += 1;
        Ok(())
    }

    fn initialize_steady_state(&mut self) -> Result<()> {
        let (a_scaled, b_scaled) = self.scaled_model_matrices();
        let u = initial_u_vector(&self.model, &self.simulation, &self.config.variables);
        let y_target = initial_y_vector(&self.simulation, &self.model.y_order);
        self.plant_state = solve_steady_state(&a_scaled, &b_scaled, &self.model, &u, &y_target)?;
        Ok(())
    }

    fn build_inputs(&self) -> HashMap<String, SignalValue> {
        let mut values = HashMap::new();
        let params = self.logic.current_params();

        for point in &self.logic_io.inputs {
            let Some(value) = self.simulated_input_value(point.name.as_str(), &params) else {
                continue;
            };
            values.insert(point.name.clone(), value);
        }

        values
    }

    fn simulated_input_value(
        &self,
        signal_name: &str,
        params: &HashMap<String, LogicParamValue>,
    ) -> Option<SignalValue> {
        let mut parts = signal_name.split('.');
        let domain = parts.next()?;
        let name = parts.next()?;
        let field = parts.next()?;
        if parts.next().is_some() {
            return None;
        }

        match (domain, field) {
            ("mv", "read") => Some(SignalValue::F64(*self.current_mv.get(name).unwrap_or(&0.0))),
            ("mv", "enable") => Some(SignalValue::Bool(true)),
            ("mv", "hl") => {
                simulation_mv_scalar(params, &self.config.io_definition.mvs, name, "hl")
                    .map(SignalValue::F64)
            }
            ("mv", "ll") => {
                simulation_mv_scalar(params, &self.config.io_definition.mvs, name, "ll")
                    .map(SignalValue::F64)
            }
            ("cv", "soll") => Some(SignalValue::F64(
                *self.simulation.cv_setpoints.get(name).unwrap_or(&0.0),
            )),
            ("cv", "ist") => Some(SignalValue::F64(*self.current_cv.get(name).unwrap_or(&0.0))),
            ("cv", "enable") => Some(SignalValue::Bool(true)),
            ("cv", "sollband") => {
                simulation_cv_scalar(params, &self.config.io_definition.cvs, name, "sollband")
                    .map(SignalValue::F64)
            }
            ("dv", "read") => Some(SignalValue::F64(*self.current_dv.get(name).unwrap_or(&0.0))),
            ("dv", "enable") => Some(SignalValue::Bool(true)),
            _ => None,
        }
    }

    fn scaled_model_matrices(&self) -> (Vec<Vec<f64>>, Vec<Vec<f64>>) {
        let mut a = self.model.a.clone();
        let mut b = self.model.b.clone();

        for (row_index, row) in a.iter_mut().enumerate() {
            for (col_index, value) in row.iter_mut().enumerate() {
                *value *= self.simulation.a_scale_pct[row_index][col_index] / 100.0;
            }
        }

        for (row_index, row) in b.iter_mut().enumerate() {
            for (col_index, value) in row.iter_mut().enumerate() {
                *value *= self.simulation.b_scale_pct[row_index][col_index] / 100.0;
            }
        }

        (a, b)
    }

    fn current_u_vector(
        &self,
        applied_mv: &HashMap<String, f64>,
        dv_values: &HashMap<String, f64>,
    ) -> Vec<f64> {
        let mut u = Vec::with_capacity(self.model.u_order.len());
        for name in &self.model.u_order {
            if let Some(value) = applied_mv.get(name) {
                u.push(*value);
            } else {
                u.push(*dv_values.get(name).unwrap_or(&0.0));
            }
        }
        u
    }

    fn update_measurements_from_output(
        &mut self,
        applied_mv: &HashMap<String, f64>,
        y_model: &[f64],
    ) {
        for (index, cv_name) in self.model.y_order.iter().enumerate() {
            let offset = self
                .simulation
                .cv_offsets
                .get(cv_name)
                .copied()
                .unwrap_or(0.0);
            let noise_std = self
                .simulation
                .cv_noise_std
                .get(cv_name)
                .copied()
                .unwrap_or(0.0);
            let noise = self.rng.gaussian(0.0, noise_std);
            self.current_cv
                .insert(cv_name.clone(), y_model[index] + offset + noise);
        }

        for mv_name in &self.config.variables.mvs {
            let base = applied_mv.get(mv_name).copied().unwrap_or(0.0);
            let noise_std = self
                .simulation
                .mv_noise_std
                .get(mv_name)
                .copied()
                .unwrap_or(0.0);
            let mut value = base + self.rng.gaussian(0.0, noise_std);

            if let Some(limit) = self.limits.values.get(&format!("mv.{}.write", mv_name)) {
                value = value.clamp(limit.min, limit.max);
            }
            self.current_mv.insert(mv_name.clone(), value);
        }

        for dv_name in &self.config.variables.dvs {
            let value = self
                .simulation
                .dv_disturbances
                .get(dv_name)
                .copied()
                .unwrap_or(0.0);
            self.current_dv.insert(dv_name.clone(), value);
        }
    }

    fn append_cycle_log(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        outputs: &HashMap<String, SignalValue>,
        user_logs: &[LogicUserLog],
    ) -> Result<()> {
        let record = json!({
            "timestamp_ms": now_ms(),
            "event": "simulation_cycle",
            "state": self.state,
            "simulation_time_s": self.simulation_time_s,
            "inputs": inputs,
            "outputs": outputs,
            "online_params": self.logic.current_params(),
            "user_logs": user_logs,
        });
        self.append_log_record(record)
    }

    fn store_online_param_samples(&self) -> Result<()> {
        let params = self.logic.current_params();
        let mut values = HashMap::new();
        for (key, value) in params {
            values.insert(key, SignalValue::from(value));
        }
        self.storage.store_signal_map("online_param", &values)
    }

    fn store_user_log_samples(&self, user_logs: &[LogicUserLog]) -> Result<()> {
        for entry in user_logs {
            for (signal, value) in &entry.values {
                self.storage.store_signal("user_log", signal, value)?;
            }
        }
        Ok(())
    }

    fn update_metrics(
        &mut self,
        cycle_start: Instant,
        duration: Duration,
        target_cycle: Option<Duration>,
    ) {
        let duration_us = duration.as_micros() as u64;
        let cycle_target_us = target_cycle.map(|value| value.as_micros() as i64);

        let jitter_us = match (self.last_cycle_start, cycle_target_us) {
            (Some(prev_start), Some(target)) => {
                let actual_interval = cycle_start
                    .saturating_duration_since(prev_start)
                    .as_micros() as i64;
                actual_interval - target
            }
            _ => 0,
        };

        if let Some(target) = cycle_target_us {
            if duration_us as i64 > target {
                self.deadline_misses += 1;
            }
        }

        self.last_duration_us = duration_us;
        self.last_jitter_us = jitter_us;
        self.last_cycle_start = Some(cycle_start);
    }

    fn append_log_record(&mut self, record: serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(&record).context("failed to encode simulation log")?;
        writeln!(self.log_file, "{encoded}").context("failed writing simulation log line")?;
        self.log_file
            .flush()
            .context("failed flushing simulation log")?;
        Ok(())
    }
}

fn current_runtime_scalar_param(
    params: &HashMap<String, LogicParamValue>,
    name: &str,
    suffix: &str,
) -> Option<f64> {
    let prefix = prefix_from_suffix(suffix);
    match params.get(&format!("io.{prefix}.{name}.{suffix}")) {
        Some(LogicParamValue::F64(value)) => Some(*value),
        _ => None,
    }
}

fn simulation_mv_scalar(
    params: &HashMap<String, LogicParamValue>,
    rows: &[crate::project_config::MvIoDefinition],
    name: &str,
    suffix: &str,
) -> Option<f64> {
    if let Some(value) = current_runtime_scalar_param(params, name, suffix) {
        return Some(value);
    }

    rows.iter()
        .find(|row| row.name == name)
        .and_then(|row| match suffix {
            "hl" => row.hl,
            "ll" => row.ll,
            _ => None,
        })
}

fn simulation_cv_scalar(
    params: &HashMap<String, LogicParamValue>,
    rows: &[crate::project_config::CvIoDefinition],
    name: &str,
    suffix: &str,
) -> Option<f64> {
    if let Some(value) = current_runtime_scalar_param(params, name, suffix) {
        return Some(value);
    }

    rows.iter()
        .find(|row| row.name == name)
        .and_then(|row| match suffix {
            "sollband" => row.sollband_value,
            _ => None,
        })
}

fn prefix_from_suffix(suffix: &str) -> &'static str {
    if suffix == "sollband" { "cv" } else { "mv" }
}

fn initial_u_vector(
    model: &LinearStateSpaceModelConfig,
    simulation: &SimulationConfig,
    vars: &crate::project_config::VariableCatalogConfig,
) -> Vec<f64> {
    let mut u = Vec::with_capacity(model.u_order.len());
    for name in &model.u_order {
        if vars.mvs.iter().any(|mv| mv == name) {
            u.push(*simulation.mv_initials.get(name).unwrap_or(&0.0));
        } else {
            u.push(*simulation.dv_initials.get(name).unwrap_or(&0.0));
        }
    }
    u
}

fn initial_y_vector(simulation: &SimulationConfig, y_order: &[String]) -> Vec<f64> {
    y_order
        .iter()
        .map(|name| {
            simulation.cv_initials.get(name).copied().unwrap_or(0.0)
                - simulation.cv_offsets.get(name).copied().unwrap_or(0.0)
        })
        .collect()
}

fn solve_steady_state(
    a: &[Vec<f64>],
    b: &[Vec<f64>],
    model: &LinearStateSpaceModelConfig,
    u: &[f64],
    y_target: &[f64],
) -> Result<Vec<f64>> {
    let nx = model.state_count;
    let ny = model.y_order.len();
    let mut matrix = Vec::with_capacity(nx + ny);
    let mut rhs = Vec::with_capacity(nx + ny);

    for row_index in 0..nx {
        matrix.push(a[row_index].clone());
        rhs.push(-dot(&b[row_index], u));
    }

    for row_index in 0..ny {
        matrix.push(model.c[row_index].clone());
        rhs.push(y_target[row_index] - dot(&model.d[row_index], u));
    }

    let solution = solve_least_squares(&matrix, &rhs)?;
    let residual = max_residual(&matrix, &solution, &rhs);
    let scale = rhs
        .iter()
        .copied()
        .fold(1.0_f64, |max, value| max.max(value.abs()));
    if residual > 1e-6 * scale.max(1.0) {
        bail!("simulation initial condition is inconsistent with the configured steady state");
    }
    Ok(solution)
}

fn solve_least_squares(matrix: &[Vec<f64>], rhs: &[f64]) -> Result<Vec<f64>> {
    let cols = matrix.first().map_or(0, Vec::len);
    let mut normal = vec![vec![0.0; cols]; cols];
    let mut normal_rhs = vec![0.0; cols];

    for (row, value) in matrix.iter().zip(rhs.iter()) {
        for col_i in 0..cols {
            normal_rhs[col_i] += row[col_i] * value;
            for col_j in 0..cols {
                normal[col_i][col_j] += row[col_i] * row[col_j];
            }
        }
    }

    solve_linear_system(normal, normal_rhs)
}

fn solve_linear_system(mut matrix: Vec<Vec<f64>>, mut rhs: Vec<f64>) -> Result<Vec<f64>> {
    let n = matrix.len();
    for pivot_index in 0..n {
        let mut best_row = pivot_index;
        let mut best_value = matrix[pivot_index][pivot_index].abs();
        for row_index in (pivot_index + 1)..n {
            let value = matrix[row_index][pivot_index].abs();
            if value > best_value {
                best_value = value;
                best_row = row_index;
            }
        }

        if best_value <= 1e-12 {
            bail!("simulation initial condition solve is singular");
        }

        if best_row != pivot_index {
            matrix.swap(best_row, pivot_index);
            rhs.swap(best_row, pivot_index);
        }

        let pivot = matrix[pivot_index][pivot_index];
        for col_index in pivot_index..n {
            matrix[pivot_index][col_index] /= pivot;
        }
        rhs[pivot_index] /= pivot;

        for row_index in 0..n {
            if row_index == pivot_index {
                continue;
            }
            let factor = matrix[row_index][pivot_index];
            if factor.abs() <= 1e-15 {
                continue;
            }
            for col_index in pivot_index..n {
                matrix[row_index][col_index] -= factor * matrix[pivot_index][col_index];
            }
            rhs[row_index] -= factor * rhs[pivot_index];
        }
    }

    Ok(rhs)
}

fn max_residual(matrix: &[Vec<f64>], solution: &[f64], rhs: &[f64]) -> f64 {
    matrix
        .iter()
        .zip(rhs.iter())
        .map(|(row, expected)| (dot(row, solution) - expected).abs())
        .fold(0.0_f64, f64::max)
}

fn rk4_step(a: &[Vec<f64>], b: &[Vec<f64>], x: &[f64], u: &[f64], dt: f64) -> Vec<f64> {
    let k1 = state_derivative(a, b, x, u);
    let k2 = state_derivative(a, b, &vec_add_scaled(x, &k1, dt * 0.5), u);
    let k3 = state_derivative(a, b, &vec_add_scaled(x, &k2, dt * 0.5), u);
    let k4 = state_derivative(a, b, &vec_add_scaled(x, &k3, dt), u);

    x.iter()
        .enumerate()
        .map(|(index, value)| {
            value + (dt / 6.0) * (k1[index] + 2.0 * k2[index] + 2.0 * k3[index] + k4[index])
        })
        .collect()
}

fn state_derivative(a: &[Vec<f64>], b: &[Vec<f64>], x: &[f64], u: &[f64]) -> Vec<f64> {
    a.iter()
        .zip(b.iter())
        .map(|(a_row, b_row)| dot(a_row, x) + dot(b_row, u))
        .collect()
}

fn output_vector(model: &LinearStateSpaceModelConfig, x: &[f64], u: &[f64]) -> Vec<f64> {
    model
        .c
        .iter()
        .zip(model.d.iter())
        .map(|(c_row, d_row)| dot(c_row, x) + dot(d_row, u))
        .collect()
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter().zip(right.iter()).map(|(l, r)| l * r).sum()
}

fn vec_add_scaled(base: &[f64], delta: &[f64], scale: f64) -> Vec<f64> {
    base.iter()
        .zip(delta.iter())
        .map(|(base_value, delta_value)| base_value + delta_value * scale)
        .collect()
}

fn apply_output_limits(limits: &LimitsConfig, outputs: &mut HashMap<String, SignalValue>) {
    for (name, value) in outputs.iter_mut() {
        if let Some(limit) = limits.values.get(name) {
            if let SignalValue::F64(v) = value {
                *v = v.clamp(limit.min, limit.max);
            }
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_millis() as i64
}

#[derive(Debug, Clone)]
struct SmallRng {
    state: u64,
    spare: Option<f64>,
}

impl SmallRng {
    fn seeded() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos() as u64;
        Self {
            state: nanos ^ 0x9E37_79B9_7F4A_7C15,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn next_unit(&mut self) -> f64 {
        let raw = self.next_u64() >> 11;
        (raw as f64) * (1.0 / ((1u64 << 53) as f64))
    }

    fn gaussian(&mut self, mean: f64, std: f64) -> f64 {
        if std <= 0.0 {
            return 0.0;
        }
        let sample = if let Some(spare) = self.spare.take() {
            spare
        } else {
            let u1 = self
                .next_unit()
                .clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON);
            let u2 = self.next_unit();
            let radius = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            self.spare = Some(radius * theta.sin());
            radius * theta.cos()
        };
        mean + std * sample
    }
}

pub(crate) fn wall_cycle_duration(speed: SimulationSpeedMode, cycle_ms: u64) -> Option<Duration> {
    let speed_factor = match speed {
        SimulationSpeedMode::RealTime => 1.0,
        SimulationSpeedMode::X0_1 => 0.1,
        SimulationSpeedMode::X0_5 => 0.5,
        SimulationSpeedMode::X10 => 10.0,
        SimulationSpeedMode::X20 => 20.0,
        SimulationSpeedMode::X50 => 50.0,
        SimulationSpeedMode::X100 => 100.0,
        SimulationSpeedMode::AsFastAsPossible => return None,
    };

    Some(Duration::from_secs_f64(
        (cycle_ms as f64 / 1000.0) / speed_factor,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_cycle_duration_scales_expected_direction() {
        let realtime = wall_cycle_duration(SimulationSpeedMode::RealTime, 1_000).unwrap();
        let faster = wall_cycle_duration(SimulationSpeedMode::X10, 1_000).unwrap();
        let slower = wall_cycle_duration(SimulationSpeedMode::X0_1, 1_000).unwrap();

        assert_eq!(realtime.as_millis(), 1_000);
        assert_eq!(faster.as_millis(), 100);
        assert_eq!(slower.as_millis(), 10_000);
        assert!(wall_cycle_duration(SimulationSpeedMode::AsFastAsPossible, 1_000).is_none());
    }

    #[test]
    fn steady_state_solver_rejects_singular_system() {
        let model = LinearStateSpaceModelConfig {
            state_count: 1,
            u_order: vec!["u1".to_string()],
            y_order: vec!["y1".to_string()],
            a: vec![vec![0.0]],
            b: vec![vec![0.0]],
            c: vec![vec![0.0]],
            d: vec![vec![0.0]],
        };
        let err = solve_steady_state(&model.a, &model.b, &model, &[0.0], &[1.0])
            .expect_err("expected singular solve");
        assert!(err.to_string().contains("singular"));
    }

    #[test]
    fn rk4_step_matches_first_order_decay() {
        let a = vec![vec![-1.0]];
        let b = vec![vec![1.0]];
        let next = rk4_step(&a, &b, &[0.0], &[1.0], 1.0);
        assert!((next[0] - 0.625).abs() < 0.01);
    }
}
