use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use nalgebra::{DMatrix, DVector};
use osqp::{CscMatrix, Problem, Settings, Status};

use crate::{
    logic::{ControlLogic, LogicMetadata, LogicUserLog, ParameterSchemaEntry},
    project_config::{
        IoConfig, LinearStateSpaceModelConfig, LogicParamValue, VariableCatalogConfig,
    },
    types::{RuntimeState, SignalValue},
};

const DEFAULT_PREDICTION_HORIZON: usize = 20;
const DEFAULT_CONTROL_HORIZON: usize = 5;
const DEFAULT_CV_Q: f64 = 1.0;
const DEFAULT_MV_R: f64 = 0.1;
const DEFAULT_MAX_DELTA_U: f64 = 1.0;
const DEFAULT_PROCESS_Q: f64 = 1.0e-4;
const DEFAULT_OUTPUT_BIAS_Q: f64 = 1.0e-3;
const DEFAULT_MEASUREMENT_R: f64 = 1.0e-2;
const DEFAULT_INITIAL_P: f64 = 1.0;
const MAX_HORIZON: usize = 200;
const MIN_WEIGHT: f64 = 1.0e-12;

#[derive(Debug, Clone)]
pub struct LinearMpcLogic {
    model: LinearStateSpaceModelConfig,
    vars: VariableCatalogConfig,
    cycle_s: f64,
    params: HashMap<String, LogicParamValue>,
    defaults: HashMap<String, LogicParamValue>,
    external_band_params: HashSet<String>,
    x_hat: DVector<f64>,
    y_bias: DVector<f64>,
    covariance: DMatrix<f64>,
    initialized: bool,
    observer_stable_cycles: usize,
    suggestion_succeeded: bool,
    shadow_mode: bool,
    last_mv: Vec<f64>,
    last_u_total: Vec<f64>,
    pending_logs: Vec<LogicUserLog>,
}

#[derive(Debug, Clone)]
pub(crate) struct DiscreteModel {
    a: DMatrix<f64>,
    b: DMatrix<f64>,
    c: DMatrix<f64>,
    d: DMatrix<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ObserverReplaySnapshot {
    pub states: Vec<f64>,
    pub biases: Vec<f64>,
    pub cv_estimates: Vec<f64>,
}

#[derive(Debug, Clone)]
struct MpcProblemData {
    base_prediction: DVector<f64>,
    target: DVector<f64>,
    phi: DMatrix<f64>,
    free_prediction: DVector<f64>,
}

#[derive(Debug, Clone)]
struct MpcSolution {
    delta_u: Vec<f64>,
    controlled_prediction: DVector<f64>,
    fallback: bool,
    status_code: f64,
    status_message: String,
}

impl LinearMpcLogic {
    pub fn new(
        model: LinearStateSpaceModelConfig,
        vars: VariableCatalogConfig,
        cycle_ms: u64,
        params: HashMap<String, LogicParamValue>,
        cv_band_defaults: HashMap<String, (f64, f64)>,
    ) -> Self {
        let defaults = linear_mpc_default_params(&model, &vars, &cv_band_defaults);
        let mv_count = vars.mvs.len();
        let u_count = model.u_order.len();
        Self {
            x_hat: DVector::zeros(model.state_count),
            y_bias: DVector::zeros(vars.cvs.len()),
            covariance: DMatrix::identity(
                model.state_count + vars.cvs.len(),
                model.state_count + vars.cvs.len(),
            ) * DEFAULT_INITIAL_P,
            last_mv: vec![0.0; mv_count],
            last_u_total: vec![0.0; u_count],
            initialized: false,
            observer_stable_cycles: 0,
            suggestion_succeeded: false,
            shadow_mode: false,
            pending_logs: Vec::new(),
            cycle_s: cycle_ms as f64 / 1000.0,
            model,
            vars,
            params,
            defaults,
            external_band_params: HashSet::new(),
        }
    }

    pub(crate) fn with_io_contract(mut self, io: &IoConfig) -> Self {
        // Keep externally driven offsets out of the parameter API, including stale saved tuning.
        for cv in &self.vars.cvs {
            for suffix in ["band_low", "band_high"] {
                if io
                    .inputs
                    .iter()
                    .any(|input| input.name == format!("cv.{cv}.{suffix}"))
                {
                    let key = format!("mpc.cv.{cv}.{suffix}");
                    self.params.remove(&key);
                    self.external_band_params.insert(key);
                }
            }
        }
        self
    }

    pub fn is_configured(&self) -> bool {
        !self.vars.mvs.is_empty()
            && !self.vars.cvs.is_empty()
            && self.model.state_count > 0
            && self.model.y_order == self.vars.cvs
            && self
                .model
                .u_order
                .iter()
                .eq(self.vars.mvs.iter().chain(self.vars.dvs.iter()))
    }

    fn schema_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .defaults
            .keys()
            .filter(|key| !self.external_band_params.contains(*key))
            .cloned()
            .collect();
        names.sort();
        names
    }

    fn f64_param(&self, key: &str, fallback: f64) -> f64 {
        match self.params.get(key).or_else(|| self.defaults.get(key)) {
            Some(LogicParamValue::F64(v)) if v.is_finite() => *v,
            _ => fallback,
        }
    }

    fn usize_param(&self, key: &str, fallback: usize) -> usize {
        self.f64_param(key, fallback as f64)
            .round()
            .clamp(1.0, MAX_HORIZON as f64) as usize
    }

    fn cv_weight(&self, cv: &str) -> f64 {
        self.f64_param(&format!("mpc.cv.{cv}.q"), DEFAULT_CV_Q)
            .max(0.0)
    }

    fn mv_weight(&self, mv: &str) -> f64 {
        self.f64_param(&format!("mpc.mv.{mv}.r"), DEFAULT_MV_R)
            .max(MIN_WEIGHT)
    }

    fn mv_uses_l1(&self, mv: &str) -> bool {
        let key = format!("mpc.mv.{mv}.use_l1");
        matches!(
            self.params.get(&key).or_else(|| self.defaults.get(&key)),
            Some(LogicParamValue::Bool(true))
        )
    }

    fn max_delta_u(&self, mv: &str) -> f64 {
        self.f64_param(&format!("mpc.mv.{mv}.max_delta_u"), DEFAULT_MAX_DELTA_U)
            .abs()
    }

    fn process_noise(&self, index: usize) -> f64 {
        self.f64_param(
            &format!("mpc.kalman.state.{}.q", index + 1),
            DEFAULT_PROCESS_Q,
        )
        .max(0.0)
    }

    fn output_bias_noise(&self, cv: &str) -> f64 {
        self.f64_param(&format!("mpc.kalman.cv.{cv}.bias_q"), DEFAULT_OUTPUT_BIAS_Q)
            .max(0.0)
    }

    fn measurement_noise(&self, cv: &str) -> f64 {
        self.f64_param(&format!("mpc.kalman.cv.{cv}.r"), DEFAULT_MEASUREMENT_R)
            .max(MIN_WEIGHT)
    }

    fn input_bool_or_default_true(inputs: &HashMap<String, SignalValue>, key: &str) -> bool {
        match inputs.get(key) {
            None => true,
            Some(SignalValue::Bool(v)) => *v,
            Some(SignalValue::F64(v)) => *v > 0.5,
        }
    }

    fn all_enabled(&self, inputs: &HashMap<String, SignalValue>) -> bool {
        self.vars
            .cvs
            .iter()
            .all(|cv| Self::input_bool_or_default_true(inputs, &format!("cv.{cv}.enable")))
            && self
                .vars
                .mvs
                .iter()
                .all(|mv| Self::input_bool_or_default_true(inputs, &format!("mv.{mv}.enable")))
            && self
                .vars
                .dvs
                .iter()
                .all(|dv| Self::input_bool_or_default_true(inputs, &format!("dv.{dv}.enable")))
    }

    fn read_cv_vector(
        &self,
        inputs: &HashMap<String, SignalValue>,
        field: &str,
    ) -> Option<Vec<f64>> {
        let mut values = Vec::with_capacity(self.vars.cvs.len());
        for cv in &self.vars.cvs {
            values.push(inputs.get(&format!("cv.{cv}.{field}"))?.as_f64()?);
        }
        Some(values)
    }

    fn current_mv_vector(&self, inputs: &HashMap<String, SignalValue>) -> Vec<f64> {
        self.vars
            .mvs
            .iter()
            .enumerate()
            .map(|(index, mv)| {
                inputs
                    .get(&format!("mv.{mv}.read"))
                    .and_then(SignalValue::as_f64)
                    .unwrap_or(self.last_mv[index])
            })
            .collect()
    }

    fn current_dv_vector(&self, inputs: &HashMap<String, SignalValue>) -> Vec<f64> {
        self.vars
            .dvs
            .iter()
            .map(|dv| {
                inputs
                    .get(&format!("dv.{dv}.read"))
                    .and_then(SignalValue::as_f64)
                    .unwrap_or(0.0)
            })
            .collect()
    }

    fn total_input_vector(&self, mv: &[f64], dv: &[f64]) -> DVector<f64> {
        let mut values = Vec::with_capacity(self.model.u_order.len());
        for name in &self.model.u_order {
            if let Some(index) = self.vars.mvs.iter().position(|mv_name| mv_name == name) {
                values.push(mv[index]);
            } else if let Some(index) = self.vars.dvs.iter().position(|dv_name| dv_name == name) {
                values.push(dv[index]);
            } else {
                values.push(0.0);
            }
        }
        DVector::from_vec(values)
    }

    fn mv_limits(&self, inputs: &HashMap<String, SignalValue>) -> Vec<(f64, f64)> {
        self.vars
            .mvs
            .iter()
            .map(|mv| {
                let ll = inputs
                    .get(&format!("mv.{mv}.ll"))
                    .and_then(SignalValue::as_f64)
                    .or_else(|| self.f64_online_param(&format!("io.mv.{mv}.ll")))
                    .unwrap_or(f64::NEG_INFINITY);
                let hl = inputs
                    .get(&format!("mv.{mv}.hl"))
                    .and_then(SignalValue::as_f64)
                    .or_else(|| self.f64_online_param(&format!("io.mv.{mv}.hl")))
                    .unwrap_or(f64::INFINITY);
                if ll <= hl { (ll, hl) } else { (hl, ll) }
            })
            .collect()
    }

    fn f64_online_param(&self, key: &str) -> Option<f64> {
        match self.params.get(key) {
            Some(LogicParamValue::F64(v)) if v.is_finite() => Some(*v),
            _ => None,
        }
    }

    fn band_edges(
        &self,
        cv: &str,
        setpoint: f64,
        inputs: &HashMap<String, SignalValue>,
    ) -> (f64, f64) {
        let offset = |suffix: &str| {
            inputs
                .get(&format!("cv.{cv}.{suffix}"))
                .and_then(SignalValue::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or_else(|| self.f64_param(&format!("mpc.cv.{cv}.{suffix}"), 0.0))
        };
        let low_offset = offset("band_low");
        let high_offset = offset("band_high");
        let low = setpoint + low_offset.min(high_offset);
        let high = setpoint + low_offset.max(high_offset);
        (low, high)
    }

    fn init_from_measurement(
        &mut self,
        y_measured: &[f64],
        u_total: &DVector<f64>,
        model: &DiscreteModel,
    ) {
        let y_without_d = DVector::from_iterator(
            y_measured.len(),
            y_measured.iter().enumerate().map(|(row, y)| {
                let d_row = model.d.row(row);
                y - d_row.dot(u_total)
            }),
        );
        let normal = model.c.transpose() * &model.c
            + DMatrix::identity(self.model.state_count, self.model.state_count) * 1.0e-9;
        let rhs = model.c.transpose() * y_without_d;
        if let Some(solution) = normal.lu().solve(&rhs) {
            self.x_hat = solution;
        }
        let y_model = &model.c * &self.x_hat + &model.d * u_total;
        self.y_bias = DVector::from_iterator(
            y_measured.len(),
            y_measured
                .iter()
                .enumerate()
                .map(|(index, y)| y - y_model[index]),
        );
        let initial_p = self
            .f64_param("mpc.kalman.initial_p", DEFAULT_INITIAL_P)
            .max(MIN_WEIGHT);
        self.covariance = DMatrix::identity(
            self.model.state_count + self.vars.cvs.len(),
            self.model.state_count + self.vars.cvs.len(),
        ) * initial_p;
        self.initialized = true;
    }

    fn update_kalman(&mut self, y_measured: &[f64], u_total: &DVector<f64>, model: &DiscreteModel) {
        let nx = self.model.state_count;
        let ny = self.vars.cvs.len();
        let nz = nx + ny;
        if !self.initialized {
            self.init_from_measurement(y_measured, u_total, model);
            return;
        }

        let q = DMatrix::from_diagonal(&DVector::from_iterator(
            nz,
            (0..nz).map(|index| {
                if index < nx {
                    self.process_noise(index)
                } else {
                    self.output_bias_noise(&self.vars.cvs[index - nx])
                }
            }),
        ));
        let r = DMatrix::from_diagonal(&DVector::from_iterator(
            ny,
            self.vars.cvs.iter().map(|cv| self.measurement_noise(cv)),
        ));

        let x_state_pred =
            &model.a * &self.x_hat + &model.b * DVector::from_vec(self.last_u_total.clone());
        let mut z_pred = DVector::zeros(nz);
        z_pred.rows_mut(0, nx).copy_from(&x_state_pred);
        z_pred.rows_mut(nx, ny).copy_from(&self.y_bias);

        let mut f = DMatrix::zeros(nz, nz);
        f.view_mut((0, 0), (nx, nx)).copy_from(&model.a);
        for index in 0..ny {
            f[(nx + index, nx + index)] = 1.0;
        }

        let mut h = DMatrix::zeros(ny, nz);
        h.view_mut((0, 0), (ny, nx)).copy_from(&model.c);
        for index in 0..ny {
            h[(index, nx + index)] = 1.0;
        }

        let previous_covariance = self.covariance.clone();
        let p_pred = &f * &self.covariance * f.transpose() + q;
        let y_direct = &model.d * u_total;
        let y_measured = DVector::from_column_slice(y_measured);
        let innovation = y_measured - y_direct - &h * &z_pred;
        let s = &h * &p_pred * h.transpose() + r;
        let rhs = &h * &p_pred;
        let Some(k_t) = s.lu().solve(&rhs) else {
            self.x_hat = x_state_pred;
            self.y_bias = z_pred.rows(nx, ny).into_owned();
            self.covariance = p_pred;
            self.observer_stable_cycles = 0;
            return;
        };
        let k = k_t.transpose();
        let z_hat = z_pred + &k * innovation;
        self.x_hat = z_hat.rows(0, nx).into_owned();
        self.y_bias = z_hat.rows(nx, ny).into_owned();
        let identity = DMatrix::identity(nz, nz);
        self.covariance = (&identity - &k * &h) * p_pred;
        // Convergence means a stable finite posterior covariance, not merely
        // initializing the state from one measurement. Require five updates.
        let relative_change = (&self.covariance - &previous_covariance).norm()
            / previous_covariance.norm().max(MIN_WEIGHT);
        let finite = self
            .covariance
            .iter()
            .chain(self.x_hat.iter())
            .chain(self.y_bias.iter())
            .all(|v| v.is_finite());
        self.observer_stable_cycles = if finite && relative_change <= 1.0e-3 {
            self.observer_stable_cycles + 1
        } else {
            0
        };
    }

    pub(crate) fn replay_observer_step(
        &mut self,
        y_measured: &[f64],
        current_mv: &[f64],
        current_dv: &[f64],
        dt_s: f64,
    ) -> Result<ObserverReplaySnapshot> {
        if !dt_s.is_finite() || dt_s <= 0.0 {
            bail!("observer replay interval must be finite and > 0");
        }
        if y_measured.len() != self.vars.cvs.len()
            || current_mv.len() != self.vars.mvs.len()
            || current_dv.len() != self.vars.dvs.len()
        {
            bail!("observer replay values do not match configured CV/MV/DV dimensions");
        }

        let discrete = discretize_rk4(&self.model, dt_s);
        let current_u_total = self.total_input_vector(current_mv, current_dv);
        self.update_kalman(y_measured, &current_u_total, &discrete);
        let cv_estimates =
            &discrete.c * &self.x_hat + &discrete.d * &current_u_total + &self.y_bias;
        self.last_u_total = current_u_total.as_slice().to_vec();

        Ok(ObserverReplaySnapshot {
            states: self.x_hat.as_slice().to_vec(),
            biases: self.y_bias.as_slice().to_vec(),
            cv_estimates: cv_estimates.as_slice().to_vec(),
        })
    }

    fn build_problem_data(
        &self,
        model: &DiscreteModel,
        current_mv: &[f64],
        current_dv: &[f64],
        bands: &[(f64, f64)],
        prediction_horizon: usize,
        control_horizon: usize,
    ) -> MpcProblemData {
        let zero_moves = vec![0.0; self.vars.mvs.len() * control_horizon];
        let base_prediction = self.predict_outputs(
            model,
            &self.x_hat,
            current_mv,
            current_dv,
            &zero_moves,
            prediction_horizon,
            control_horizon,
        );
        let free_prediction = base_prediction.clone();

        let mut target = base_prediction.clone();
        for step in 0..prediction_horizon {
            for (cv_index, &(low, high)) in bands.iter().enumerate() {
                let row = step * self.vars.cvs.len() + cv_index;
                target[row] = base_prediction[row].clamp(low, high);
            }
        }

        let variable_count = self.vars.mvs.len() * control_horizon;
        let mut phi = DMatrix::zeros(prediction_horizon * self.vars.cvs.len(), variable_count);
        for column in 0..variable_count {
            let mut moves = vec![0.0; variable_count];
            moves[column] = 1.0;
            let response = self.predict_outputs(
                model,
                &self.x_hat,
                current_mv,
                current_dv,
                &moves,
                prediction_horizon,
                control_horizon,
            );
            for row in 0..response.len() {
                phi[(row, column)] = response[row] - base_prediction[row];
            }
        }

        MpcProblemData {
            base_prediction,
            target,
            phi,
            free_prediction,
        }
    }

    fn predict_outputs(
        &self,
        model: &DiscreteModel,
        initial_state: &DVector<f64>,
        current_mv: &[f64],
        current_dv: &[f64],
        delta_moves: &[f64],
        prediction_horizon: usize,
        control_horizon: usize,
    ) -> DVector<f64> {
        let ny = self.vars.cvs.len();
        let mv_count = self.vars.mvs.len();
        let mut out = DVector::zeros(prediction_horizon * ny);
        let mut x = initial_state.clone();
        let mut cumulative_du = vec![0.0; mv_count];

        for step in 0..prediction_horizon {
            if step < control_horizon {
                for mv_index in 0..mv_count {
                    cumulative_du[mv_index] += delta_moves[step * mv_count + mv_index];
                }
            }

            let mut mv = current_mv.to_vec();
            for (index, value) in mv.iter_mut().enumerate() {
                *value += cumulative_du[index];
            }
            let u_total = self.total_input_vector(&mv, current_dv);
            let y = &model.c * &x + &model.d * &u_total + &self.y_bias;
            for cv_index in 0..ny {
                out[step * ny + cv_index] = y[cv_index];
            }
            x = &model.a * x + &model.b * u_total;
        }

        out
    }

    fn solve_mpc(
        &self,
        data: &MpcProblemData,
        current_mv: &[f64],
        limits: &[(f64, f64)],
        control_horizon: usize,
    ) -> Result<MpcSolution> {
        let mv_count = self.vars.mvs.len();
        let variable_count = mv_count * control_horizon;
        if variable_count == 0 {
            bail!("Linear MPC requires at least one MV");
        }

        let q_diag = DVector::from_iterator(
            data.base_prediction.len(),
            (0..data.base_prediction.len()).map(|row| {
                let cv_index = row % self.vars.cvs.len();
                self.cv_weight(&self.vars.cvs[cv_index])
            }),
        );
        let mut weighted_phi = data.phi.clone();
        for row in 0..weighted_phi.nrows() {
            weighted_phi.row_mut(row).scale_mut(q_diag[row]);
        }
        let mut hessian = data.phi.transpose() * weighted_phi;
        let mut l1_moves = Vec::new();
        for step in 0..control_horizon {
            for (mv_index, mv) in self.vars.mvs.iter().enumerate() {
                let variable = step * mv_count + mv_index;
                if self.mv_uses_l1(mv) {
                    l1_moves.push((variable, self.mv_weight(mv)));
                } else {
                    hessian[(variable, variable)] += self.mv_weight(mv);
                }
            }
        }
        let mut gradient = 2.0
            * data.phi.transpose()
            * DVector::from_iterator(
                data.base_prediction.len(),
                (0..data.base_prediction.len())
                    .map(|row| q_diag[row] * (data.base_prediction[row] - data.target[row])),
            );
        let mut p = hessian * 2.0;

        let (mut constraint_matrix, mut lower, mut upper) =
            self.constraint_matrix(current_mv, limits, control_horizon);
        if !l1_moves.is_empty() {
            // Each L1 move gets an epigraph variable t >= |delta_u|. Its linear R*t
            // cost preserves convexity without adding a squared move cost for that MV.
            // Keep the original matrix construction unchanged for all-L2 projects.
            let augmented_count = variable_count + l1_moves.len();
            let original_rows = constraint_matrix.nrows();
            let mut augmented_p = DMatrix::zeros(augmented_count, augmented_count);
            augmented_p
                .view_mut((0, 0), (variable_count, variable_count))
                .copy_from(&p);
            let mut augmented_gradient = DVector::zeros(augmented_count);
            augmented_gradient
                .rows_mut(0, variable_count)
                .copy_from(&gradient);
            let mut augmented_constraints =
                DMatrix::zeros(original_rows + 2 * l1_moves.len(), augmented_count);
            augmented_constraints
                .view_mut((0, 0), (original_rows, variable_count))
                .copy_from(&constraint_matrix);

            for (index, &(variable, weight)) in l1_moves.iter().enumerate() {
                let auxiliary = variable_count + index;
                augmented_gradient[auxiliary] = weight;
                for (offset, sign) in [(0, 1.0), (1, -1.0)] {
                    let row = original_rows + 2 * index + offset;
                    augmented_constraints[(row, variable)] = sign;
                    augmented_constraints[(row, auxiliary)] = -1.0;
                    lower.push(f64::NEG_INFINITY);
                    upper.push(0.0);
                }
            }
            p = augmented_p;
            gradient = augmented_gradient;
            constraint_matrix = augmented_constraints;
        }
        let p_rows = p.nrows();
        let p_cols = p.ncols();
        let a_rows = constraint_matrix.nrows();
        let a_cols = constraint_matrix.ncols();
        let settings = Settings::default().verbose(false);
        let mut problem = Problem::new(
            CscMatrix::from_row_iter_dense(p_rows, p_cols, row_major_values(&p)).into_upper_tri(),
            gradient.as_slice(),
            CscMatrix::from_row_iter_dense(a_rows, a_cols, row_major_values(&constraint_matrix)),
            lower.as_slice(),
            upper.as_slice(),
            &settings,
        )
        .context("failed to set up OSQP problem")?;
        let result = problem.solve();
        let (solution, status_message, status_code) = match result {
            Status::Solved(solution) => (solution.x().to_vec(), "osqp_solved".to_string(), 1.0),
            Status::SolvedInaccurate(solution) => (
                solution.x().to_vec(),
                "osqp_solved_inaccurate".to_string(),
                0.5,
            ),
            Status::MaxIterationsReached(solution) => (
                solution.x().to_vec(),
                "osqp_max_iterations_reached".to_string(),
                0.25,
            ),
            Status::TimeLimitReached(solution) => (
                solution.x().to_vec(),
                "osqp_time_limit_reached".to_string(),
                0.25,
            ),
            other => bail!("OSQP failed with status {other:?}"),
        };
        // Auxiliary absolute-value variables are solver internals, not actuator moves.
        let delta_u = solution[..variable_count].to_vec();
        let controlled_prediction =
            &data.base_prediction + &data.phi * DVector::from_vec(delta_u.clone());

        Ok(MpcSolution {
            delta_u,
            controlled_prediction,
            fallback: false,
            status_code,
            status_message,
        })
    }

    fn constraint_matrix(
        &self,
        current_mv: &[f64],
        limits: &[(f64, f64)],
        control_horizon: usize,
    ) -> (DMatrix<f64>, Vec<f64>, Vec<f64>) {
        let mv_count = self.vars.mvs.len();
        let variable_count = mv_count * control_horizon;
        let rows = variable_count + variable_count;
        let mut matrix = DMatrix::zeros(rows, variable_count);
        let mut lower = vec![f64::NEG_INFINITY; rows];
        let mut upper = vec![f64::INFINITY; rows];

        for step in 0..control_horizon {
            for (mv_index, mv) in self.vars.mvs.iter().enumerate() {
                let variable = step * mv_count + mv_index;
                matrix[(variable, variable)] = 1.0;
                let max_delta = self.max_delta_u(mv);
                lower[variable] = -max_delta;
                upper[variable] = max_delta;

                let row = variable_count + variable;
                for prior_step in 0..=step {
                    matrix[(row, prior_step * mv_count + mv_index)] = 1.0;
                }
                lower[row] = limits[mv_index].0 - current_mv[mv_index];
                upper[row] = limits[mv_index].1 - current_mv[mv_index];
            }
        }

        (matrix, lower, upper)
    }

    fn fallback_solution(
        &self,
        data: &MpcProblemData,
        current_mv: &[f64],
        limits: &[(f64, f64)],
        reason: String,
    ) -> MpcSolution {
        let mv_count = self.vars.mvs.len();
        let mut delta_u = vec![0.0; mv_count];
        for (index, value) in delta_u.iter_mut().enumerate() {
            let clamped = current_mv[index].clamp(limits[index].0, limits[index].1);
            *value = clamped - current_mv[index];
        }
        MpcSolution {
            delta_u,
            controlled_prediction: data.free_prediction.clone(),
            fallback: true,
            status_code: -1.0,
            status_message: reason,
        }
    }

    fn emit_logs(
        &mut self,
        solution: &MpcSolution,
        data: &MpcProblemData,
        bands: &[(f64, f64)],
        next_mv: &[f64],
    ) {
        let mut values = HashMap::new();
        for index in 0..self.x_hat.len() {
            values.insert(
                format!("mpc.xhat.{}", index + 1),
                SignalValue::F64(self.x_hat[index]),
            );
        }
        for (index, cv) in self.vars.cvs.iter().enumerate() {
            values.insert(
                format!("mpc.bias.cv.{cv}"),
                SignalValue::F64(self.y_bias[index]),
            );
        }
        for (index, mv) in self.vars.mvs.iter().enumerate() {
            values.insert(
                format!("mpc.du.{mv}"),
                SignalValue::F64(solution.delta_u.get(index).copied().unwrap_or(0.0)),
            );
            values.insert(
                format!("mpc.target.mv.{mv}"),
                SignalValue::F64(next_mv[index]),
            );
        }
        for (cv_index, cv) in self.vars.cvs.iter().enumerate() {
            let (low, high) = bands[cv_index];
            values.insert(format!("mpc.target.cv.{cv}.low"), SignalValue::F64(low));
            values.insert(format!("mpc.target.cv.{cv}.high"), SignalValue::F64(high));
        }
        let horizon = data.free_prediction.len() / self.vars.cvs.len();
        for step in 0..horizon {
            for (cv_index, cv) in self.vars.cvs.iter().enumerate() {
                let key_suffix = format!("{}.k{:02}", cv, step + 1);
                let row = step * self.vars.cvs.len() + cv_index;
                let free_prediction = data.free_prediction[row];
                let controlled_prediction = solution.controlled_prediction[row];
                values.insert(
                    format!("mpc.pred.free.{key_suffix}"),
                    SignalValue::F64(free_prediction),
                );
                values.insert(
                    format!("mpc.pred.mpc.{key_suffix}"),
                    SignalValue::F64(controlled_prediction),
                );
                values.insert(
                    format!("future_pred_without_control.cv.{key_suffix}"),
                    SignalValue::F64(free_prediction),
                );
                values.insert(
                    format!("future_pred_with_control.cv.{key_suffix}"),
                    SignalValue::F64(controlled_prediction),
                );
            }
        }
        for (mv_index, mv) in self.vars.mvs.iter().enumerate() {
            let mut predicted_mv = next_mv[mv_index];
            for step in 0..horizon {
                if step > 0 && step < horizon {
                    let delta_index = step * self.vars.mvs.len() + mv_index;
                    if let Some(delta) = solution.delta_u.get(delta_index) {
                        predicted_mv += delta;
                    }
                }
                values.insert(
                    format!("future_pred_with_control.mv.{mv}.k{:02}", step + 1),
                    SignalValue::F64(predicted_mv),
                );
            }
        }
        values.insert(
            "mpc.status.ok".to_string(),
            SignalValue::Bool(!solution.fallback),
        );
        values.insert(
            "mpc.status.fallback".to_string(),
            SignalValue::Bool(solution.fallback),
        );
        values.insert(
            "mpc.status.code".to_string(),
            SignalValue::F64(solution.status_code),
        );

        self.pending_logs.push(LogicUserLog {
            message: solution.status_message.clone(),
            values,
        });
    }
}

impl ControlLogic for LinearMpcLogic {
    fn set_shadow_mode(&mut self, shadow: bool) {
        self.shadow_mode = shadow;
    }
    fn outputs_applied(&mut self, outputs: &HashMap<String, SignalValue>) {
        for (index, mv) in self.vars.mvs.iter().enumerate() {
            if let Some(value) = outputs
                .get(&format!("mv.{mv}.write"))
                .and_then(SignalValue::as_f64)
            {
                self.last_mv[index] = value;
                self.last_u_total[index] = value;
            }
        }
    }
    fn observer_converged(&self) -> bool {
        self.observer_stable_cycles >= 5
    }
    fn suggestion_succeeded(&self) -> bool {
        self.suggestion_succeeded
    }

    fn metadata(&self) -> LogicMetadata {
        LogicMetadata {
            kind: "linear_mpc".to_string(),
            description:
                "Linear state-space MPC with delta-u constraints and Kalman state estimation"
                    .to_string(),
        }
    }

    fn init(
        &mut self,
        initial_values: HashMap<String, SignalValue>,
        params: HashMap<String, LogicParamValue>,
    ) {
        self.set_params(params);
        for (index, mv) in self.vars.mvs.iter().enumerate() {
            if let Some(value) = initial_values
                .get(&format!("mv.{mv}.write"))
                .and_then(SignalValue::as_f64)
            {
                self.last_mv[index] = value;
            }
        }
        self.last_u_total = self
            .total_input_vector(&self.last_mv, &vec![0.0; self.vars.dvs.len()])
            .as_slice()
            .to_vec();
        self.x_hat.fill(0.0);
        self.y_bias.fill(0.0);
        self.initialized = false;
        self.observer_stable_cycles = 0;
        self.suggestion_succeeded = false;
        self.pending_logs.clear();
    }

    fn step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        _state: RuntimeState,
    ) -> Result<HashMap<String, SignalValue>> {
        self.pending_logs.clear();
        self.suggestion_succeeded = false;
        if !self.all_enabled(inputs) {
            return Ok(HashMap::new());
        }
        let Some(y_measured) = self.read_cv_vector(inputs, "process_value") else {
            return Ok(HashMap::new());
        };
        let Some(setpoints) = self.read_cv_vector(inputs, "setpoint") else {
            return Ok(HashMap::new());
        };

        let discrete = discretize_rk4(&self.model, self.cycle_s);
        let current_mv = self.current_mv_vector(inputs);
        let current_dv = self.current_dv_vector(inputs);
        let current_u_total = self.total_input_vector(&current_mv, &current_dv);
        self.update_kalman(&y_measured, &current_u_total, &discrete);

        let prediction_horizon =
            self.usize_param("mpc.prediction_horizon", DEFAULT_PREDICTION_HORIZON);
        let control_horizon = self
            .usize_param("mpc.control_horizon", DEFAULT_CONTROL_HORIZON)
            .min(prediction_horizon);
        let limits = self.mv_limits(inputs);
        // Resolve once so the optimizer and its diagnostic log use the same live band.
        let bands: Vec<_> = self
            .vars
            .cvs
            .iter()
            .zip(&setpoints)
            .map(|(cv, setpoint)| self.band_edges(cv, *setpoint, inputs))
            .collect();
        let data = self.build_problem_data(
            &discrete,
            &current_mv,
            &current_dv,
            &bands,
            prediction_horizon,
            control_horizon,
        );
        let solution = self
            .solve_mpc(&data, &current_mv, &limits, control_horizon)
            .unwrap_or_else(|err| {
                self.fallback_solution(&data, &current_mv, &limits, err.to_string())
            });

        let mut outputs = HashMap::new();
        let mut next_mv = current_mv.clone();
        for (index, mv) in self.vars.mvs.iter().enumerate() {
            let du = solution.delta_u.get(index).copied().unwrap_or(0.0);
            next_mv[index] = (current_mv[index] + du).clamp(limits[index].0, limits[index].1);
            outputs.insert(format!("mv.{mv}.write"), SignalValue::F64(next_mv[index]));
        }

        self.suggestion_succeeded = !solution.fallback;
        self.emit_logs(&solution, &data, &bands, &next_mv);
        if !self.shadow_mode {
            self.last_mv = next_mv;
        }
        self.last_u_total = self
            .total_input_vector(
                if self.shadow_mode {
                    &current_mv
                } else {
                    &self.last_mv
                },
                &current_dv,
            )
            .as_slice()
            .to_vec();
        Ok(outputs)
    }

    fn parameter_schema(&self) -> Vec<ParameterSchemaEntry> {
        self.schema_names()
            .into_iter()
            .filter_map(|name| {
                let default = self.defaults.get(&name)?.clone();
                Some(ParameterSchemaEntry {
                    description: linear_mpc_param_description(&name),
                    name,
                    param_type: default.param_type(),
                    default,
                })
            })
            .collect()
    }

    fn set_params(&mut self, params: HashMap<String, LogicParamValue>) {
        for (key, value) in params {
            if self.external_band_params.contains(&key) {
                continue;
            }
            if key.starts_with("io.") {
                self.params.insert(key, value);
                continue;
            }
            if self
                .defaults
                .get(&key)
                .is_some_and(|default| value.matches_type(default.param_type()))
            {
                self.params.insert(key, value);
            }
        }
    }

    fn current_params(&self) -> HashMap<String, LogicParamValue> {
        let mut out = self.defaults.clone();
        for (key, value) in &self.params {
            if key.starts_with("io.") || self.defaults.contains_key(key) {
                out.insert(key.clone(), value.clone());
            }
        }
        out.retain(|key, _| !self.external_band_params.contains(key));
        out
    }

    fn drain_user_logs(&mut self) -> Vec<LogicUserLog> {
        std::mem::take(&mut self.pending_logs)
    }
}

pub fn linear_mpc_default_params(
    model: &LinearStateSpaceModelConfig,
    vars: &VariableCatalogConfig,
    cv_band_defaults: &HashMap<String, (f64, f64)>,
) -> HashMap<String, LogicParamValue> {
    let mut defaults = HashMap::new();
    defaults.insert(
        "mpc.prediction_horizon".to_string(),
        LogicParamValue::F64(DEFAULT_PREDICTION_HORIZON as f64),
    );
    defaults.insert(
        "mpc.control_horizon".to_string(),
        LogicParamValue::F64(DEFAULT_CONTROL_HORIZON as f64),
    );
    defaults.insert(
        "mpc.kalman.initial_p".to_string(),
        LogicParamValue::F64(DEFAULT_INITIAL_P),
    );

    for cv in &vars.cvs {
        let (low, high) = cv_band_defaults.get(cv).copied().unwrap_or((0.0, 0.0));
        defaults.insert(format!("mpc.cv.{cv}.q"), LogicParamValue::F64(DEFAULT_CV_Q));
        defaults.insert(format!("mpc.cv.{cv}.band_low"), LogicParamValue::F64(low));
        defaults.insert(format!("mpc.cv.{cv}.band_high"), LogicParamValue::F64(high));
        defaults.insert(
            format!("mpc.kalman.cv.{cv}.r"),
            LogicParamValue::F64(DEFAULT_MEASUREMENT_R),
        );
        defaults.insert(
            format!("mpc.kalman.cv.{cv}.bias_q"),
            LogicParamValue::F64(DEFAULT_OUTPUT_BIAS_Q),
        );
    }

    for mv in &vars.mvs {
        defaults.insert(format!("mpc.mv.{mv}.r"), LogicParamValue::F64(DEFAULT_MV_R));
        defaults.insert(format!("mpc.mv.{mv}.use_l1"), LogicParamValue::Bool(false));
        defaults.insert(
            format!("mpc.mv.{mv}.max_delta_u"),
            LogicParamValue::F64(DEFAULT_MAX_DELTA_U),
        );
    }

    for index in 0..model.state_count {
        defaults.insert(
            format!("mpc.kalman.state.{}.q", index + 1),
            LogicParamValue::F64(DEFAULT_PROCESS_Q),
        );
    }

    defaults
}

pub(crate) fn discretize_rk4(model: &LinearStateSpaceModelConfig, dt: f64) -> DiscreteModel {
    let nx = model.state_count;
    let nu = model.u_order.len();
    let a_c = DMatrix::from_fn(nx, nx, |r, c| model.a[r][c]);
    let b_c = DMatrix::from_fn(nx, nu, |r, c| model.b[r][c]);
    let c = DMatrix::from_fn(model.y_order.len(), nx, |r, c| model.c[r][c]);
    let d = DMatrix::from_fn(model.y_order.len(), nu, |r, c| model.d[r][c]);

    let mut a_d = DMatrix::zeros(nx, nx);
    for col in 0..nx {
        let mut x = DVector::zeros(nx);
        x[col] = 1.0;
        let next = rk4_step_matrix(&a_c, &b_c, &x, &DVector::zeros(nu), dt);
        for row in 0..nx {
            a_d[(row, col)] = next[row];
        }
    }

    let mut b_d = DMatrix::zeros(nx, nu);
    for col in 0..nu {
        let mut u = DVector::zeros(nu);
        u[col] = 1.0;
        let next = rk4_step_matrix(&a_c, &b_c, &DVector::zeros(nx), &u, dt);
        for row in 0..nx {
            b_d[(row, col)] = next[row];
        }
    }

    DiscreteModel {
        a: a_d,
        b: b_d,
        c,
        d,
    }
}

fn rk4_step_matrix(
    a: &DMatrix<f64>,
    b: &DMatrix<f64>,
    x: &DVector<f64>,
    u: &DVector<f64>,
    dt: f64,
) -> DVector<f64> {
    let f = |state: &DVector<f64>| a * state + b * u;
    let k1 = f(x);
    let k2 = f(&(x + &k1 * (dt * 0.5)));
    let k3 = f(&(x + &k2 * (dt * 0.5)));
    let k4 = f(&(x + &k3 * dt));
    x + (k1 + k2 * 2.0 + k3 * 2.0 + k4) * (dt / 6.0)
}

fn row_major_values(matrix: &DMatrix<f64>) -> Vec<f64> {
    let mut values = Vec::with_capacity(matrix.nrows() * matrix.ncols());
    for row in 0..matrix.nrows() {
        for col in 0..matrix.ncols() {
            values.push(matrix[(row, col)]);
        }
    }
    values
}

fn linear_mpc_param_description(name: &str) -> String {
    if name == "mpc.prediction_horizon" {
        "Linear MPC prediction horizon in control cycles".to_string()
    } else if name == "mpc.control_horizon" {
        "Linear MPC control horizon in control cycles".to_string()
    } else if name == "mpc.kalman.initial_p" {
        "Initial Kalman covariance diagonal value".to_string()
    } else if name.contains(".band_low") {
        "Relative low CV band offset from the live setpoint".to_string()
    } else if name.contains(".band_high") {
        "Relative high CV band offset from the live setpoint".to_string()
    } else if name.contains(".max_delta_u") {
        "Maximum absolute MV move per control cycle".to_string()
    } else if name.contains(".kalman.state.") {
        "Kalman process noise variance for one state".to_string()
    } else if name.contains(".kalman.cv.") {
        if name.ends_with(".bias_q") {
            "Kalman random-walk process noise for one CV output bias".to_string()
        } else {
            "Kalman measurement noise variance for one CV".to_string()
        }
    } else if name.contains(".cv.") && name.ends_with(".q") {
        "CV tracking weight outside the soft band".to_string()
    } else if name.contains(".mv.") && name.ends_with(".r") {
        "MV move weight: scales the selected squared (L2) or absolute (L1) movement cost"
            .to_string()
    } else if name.contains(".mv.") && name.ends_with(".use_l1") {
        "MV movement cost: false uses squared movement (L2); true uses absolute movement (L1). Changing modes may require retuning the move weight.".to_string()
    } else {
        format!("Linear MPC parameter '{name}'")
    }
}

#[cfg(test)]
mod tests {
    use crate::project_config::LogicParamType;

    use super::*;

    fn one_by_one_logic() -> LinearMpcLogic {
        let model = LinearStateSpaceModelConfig {
            state_count: 1,
            u_order: vec!["u1".to_string()],
            y_order: vec!["y1".to_string()],
            a: vec![vec![-1.0]],
            b: vec![vec![1.0]],
            c: vec![vec![1.0]],
            d: vec![vec![0.0]],
        };
        let vars = VariableCatalogConfig {
            mvs: vec!["u1".to_string()],
            cvs: vec!["y1".to_string()],
            dvs: Vec::new(),
        };
        LinearMpcLogic::new(model, vars, 1000, HashMap::new(), HashMap::new())
    }

    #[test]
    fn readiness_requires_stable_kalman_covariance_and_a_successful_suggestion() {
        let mut logic = one_by_one_logic();
        let inputs = HashMap::from([
            ("cv.y1.process_value".into(), SignalValue::F64(0.0)),
            ("cv.y1.setpoint".into(), SignalValue::F64(1.0)),
            ("mv.u1.read".into(), SignalValue::F64(0.0)),
        ]);
        logic.init(HashMap::new(), HashMap::new());
        logic.set_shadow_mode(true);
        logic.step(&inputs, RuntimeState::Running).unwrap();
        assert!(!logic.observer_converged());
        assert!(logic.suggestion_succeeded());
        for _ in 0..200 {
            logic.step(&inputs, RuntimeState::Running).unwrap();
        }
        assert!(logic.observer_converged());
        assert!(logic.suggestion_succeeded());
        assert_eq!(
            logic.last_mv,
            vec![0.0],
            "shadow suggestions are not applied feedback"
        );
        let mut disabled = inputs.clone();
        disabled.insert("cv.y1.enable".into(), SignalValue::Bool(false));
        assert!(
            logic
                .step(&disabled, RuntimeState::Running)
                .unwrap()
                .is_empty()
        );
        assert!(!logic.suggestion_succeeded());
        logic.init(HashMap::new(), HashMap::new());
        assert!(!logic.observer_converged());
    }

    // Identity predictions isolate the move objective from discretization/observer behavior.
    fn independent_move_problem(targets: &[f64]) -> MpcProblemData {
        MpcProblemData {
            base_prediction: DVector::zeros(targets.len()),
            target: DVector::from_column_slice(targets),
            phi: DMatrix::identity(targets.len(), targets.len()),
            free_prediction: DVector::zeros(targets.len()),
        }
    }

    #[test]
    fn missing_and_explicit_l2_match_the_existing_squared_objective() {
        let mut logic = one_by_one_logic();
        logic.set_params(HashMap::from([(
            "mpc.mv.u1.r".to_string(),
            LogicParamValue::F64(1.0),
        )]));
        let data = independent_move_problem(&[0.25]);
        let missing = logic
            .solve_mpc(&data, &[0.0], &[(-10.0, 10.0)], 1)
            .expect("L2");
        // min (du - 0.25)^2 + du^2 has the analytical minimizer du = 0.125.
        assert!((missing.delta_u[0] - 0.125).abs() < 1e-3);
        logic.set_params(HashMap::from([(
            "mpc.mv.u1.use_l1".to_string(),
            LogicParamValue::Bool(false),
        )]));
        let explicit = logic
            .solve_mpc(&data, &[0.0], &[(-10.0, 10.0)], 1)
            .expect("L2");
        assert_eq!(missing.delta_u, explicit.delta_u);
        assert_eq!(
            missing.controlled_prediction,
            explicit.controlled_prediction
        );
    }

    #[test]
    fn l1_matches_analytical_zero_threshold_and_signed_moves() {
        let mut logic = one_by_one_logic();
        logic.set_params(HashMap::from([
            ("mpc.mv.u1.use_l1".to_string(), LogicParamValue::Bool(true)),
            ("mpc.mv.u1.r".to_string(), LogicParamValue::F64(1.0)),
            (
                "mpc.mv.u1.max_delta_u".to_string(),
                LogicParamValue::F64(10.0),
            ),
        ]));
        // min (du - target)^2 + |du| gives soft thresholding at |target| = 0.5.
        for (target, expected) in [(0.25, 0.0), (-0.25, 0.0), (2.0, 1.5), (-2.0, -1.5)] {
            let data = independent_move_problem(&[target]);
            let solution = logic
                .solve_mpc(&data, &[0.0], &[(-10.0, 10.0)], 1)
                .expect("L1");
            assert_eq!(
                solution.delta_u.len(),
                1,
                "exclude the absolute-value variable"
            );
            assert!(
                (solution.delta_u[0] - expected).abs() < 2e-3,
                "target={target}, solution={solution:?}"
            );
            assert!((solution.controlled_prediction[0] - expected).abs() < 2e-3);
        }
    }

    #[test]
    fn mixed_movement_costs_preserve_horizon_predictions_and_limits() {
        let mut logic = one_by_one_logic();
        logic.vars.mvs.push("u2".to_string());
        logic.model.u_order.push("u2".to_string());
        logic.model.b[0].push(1.0);
        logic.model.d[0].push(0.0);
        // Rebuild defaults for the added MV so online updates remain schema checked.
        logic.defaults = linear_mpc_default_params(&logic.model, &logic.vars, &HashMap::new());
        logic.set_params(HashMap::from([
            ("mpc.mv.u1.use_l1".to_string(), LogicParamValue::Bool(true)),
            ("mpc.mv.u1.r".to_string(), LogicParamValue::F64(1.0)),
            ("mpc.mv.u2.r".to_string(), LogicParamValue::F64(1.0)),
            (
                "mpc.mv.u1.max_delta_u".to_string(),
                LogicParamValue::F64(10.0),
            ),
            (
                "mpc.mv.u2.max_delta_u".to_string(),
                LogicParamValue::F64(10.0),
            ),
        ]));
        let data = independent_move_problem(&[0.25, 0.25, 2.0, 2.0]);
        let solution = logic
            .solve_mpc(&data, &[0.0, 0.0], &[(-10.0, 10.0); 2], 2)
            .expect("mixed norms");
        assert_eq!(solution.delta_u.len(), 4);
        assert_eq!(solution.controlled_prediction.len(), 4);
        for (actual, expected) in solution.delta_u.iter().zip([0.0, 0.125, 1.5, 1.0]) {
            assert!((actual - expected).abs() < 2e-3, "solution={solution:?}");
        }
        assert_eq!(
            solution.controlled_prediction.as_slice(),
            solution.delta_u.as_slice()
        );

        logic.set_params(HashMap::from([(
            "mpc.mv.u1.max_delta_u".to_string(),
            LogicParamValue::F64(0.1),
        )]));
        let constrained = logic
            .solve_mpc(
                &independent_move_problem(&[2.0; 4]),
                &[0.0, 0.0],
                &[(-0.4, 0.4), (-0.3, 0.3)],
                2,
            )
            .expect("constrained mixed norms");
        for (actual, expected) in constrained.delta_u.iter().zip([0.1, 0.15, 0.1, 0.15]) {
            assert!((actual - expected).abs() < 2e-3, "solution={constrained:?}");
        }
    }

    #[test]
    fn movement_cost_parameter_uses_a_boolean_schema_and_rejects_wrong_types() {
        let mut logic = one_by_one_logic();
        let entry = logic
            .parameter_schema()
            .into_iter()
            .find(|entry| entry.name == "mpc.mv.u1.use_l1")
            .expect("norm schema");
        assert_eq!(entry.param_type, LogicParamType::Bool);
        assert_eq!(entry.default, LogicParamValue::Bool(false));
        logic.set_params(HashMap::from([(
            "mpc.mv.u1.use_l1".to_string(),
            LogicParamValue::F64(1.0),
        )]));
        assert_eq!(
            logic.current_params()["mpc.mv.u1.use_l1"],
            LogicParamValue::Bool(false)
        );
        logic.set_params(HashMap::from([
            ("mpc.mv.u1.use_l1".to_string(), LogicParamValue::Bool(true)),
            ("mpc.mv.u1.r".to_string(), LogicParamValue::Bool(true)),
        ]));
        assert_eq!(
            logic.current_params()["mpc.mv.u1.use_l1"],
            LogicParamValue::Bool(true)
        );
        assert_eq!(
            logic.current_params()["mpc.mv.u1.r"],
            LogicParamValue::F64(DEFAULT_MV_R)
        );
    }

    #[test]
    fn live_movement_cost_change_applies_without_resetting_the_observer() {
        let mut logic = one_by_one_logic();
        // Pure feedthrough makes each commanded move's cost analytically observable.
        logic.model.c[0][0] = 0.0;
        logic.model.b[0][0] = 0.0;
        logic.model.d[0][0] = 1.0;
        logic.set_params(HashMap::from([
            (
                "mpc.prediction_horizon".to_string(),
                LogicParamValue::F64(1.0),
            ),
            ("mpc.control_horizon".to_string(), LogicParamValue::F64(1.0)),
            ("mpc.mv.u1.r".to_string(), LogicParamValue::F64(1.0)),
        ]));
        let inputs = HashMap::from([
            ("cv.y1.setpoint".to_string(), SignalValue::F64(0.25)),
            ("cv.y1.process_value".to_string(), SignalValue::F64(0.0)),
            ("mv.u1.read".to_string(), SignalValue::F64(0.0)),
        ]);
        let l2 = logic.step(&inputs, RuntimeState::Running).expect("L2 step");
        assert!((l2["mv.u1.write"].as_f64().unwrap() - 0.125).abs() < 1e-3);
        let before = logic.clone();
        logic.set_params(HashMap::from([(
            "mpc.mv.u1.use_l1".to_string(),
            LogicParamValue::Bool(true),
        )]));
        assert!(logic.initialized);
        assert_eq!(logic.x_hat, before.x_hat);
        assert_eq!(logic.y_bias, before.y_bias);
        assert_eq!(logic.covariance, before.covariance);
        assert_eq!(logic.last_mv, before.last_mv);
        let l1 = logic.step(&inputs, RuntimeState::Running).expect("L1 step");
        assert!(l1["mv.u1.write"].as_f64().unwrap().abs() < 2e-3);
        logic.set_params(HashMap::from([(
            "mpc.mv.u1.use_l1".to_string(),
            LogicParamValue::Bool(false),
        )]));
        let l2_again = logic
            .step(&inputs, RuntimeState::Running)
            .expect("L2 again");
        assert!((l2_again["mv.u1.write"].as_f64().unwrap() - 0.125).abs() < 1e-3);
    }

    #[test]
    fn rk4_discretization_matches_first_order_reference() {
        let logic = one_by_one_logic();
        let d = discretize_rk4(&logic.model, 1.0);
        assert!((d.a[(0, 0)] - 0.375).abs() < 1.0e-12);
        assert!((d.b[(0, 0)] - 0.625).abs() < 1.0e-12);
    }

    #[test]
    fn qp_respects_max_delta_u() {
        let mut logic = one_by_one_logic();
        logic.params.insert(
            "mpc.mv.u1.max_delta_u".to_string(),
            LogicParamValue::F64(0.25),
        );
        logic
            .params
            .insert("mpc.mv.u1.r".to_string(), LogicParamValue::F64(1.0e-6));
        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.setpoint".to_string(), SignalValue::F64(10.0));
        inputs.insert("cv.y1.process_value".to_string(), SignalValue::F64(0.0));
        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        let u = outputs
            .get("mv.u1.write")
            .and_then(SignalValue::as_f64)
            .expect("u");
        assert!(u <= 0.250001);
    }

    #[test]
    fn qp_respects_mv_limits() {
        let mut logic = one_by_one_logic();
        logic.params.insert(
            "mpc.mv.u1.max_delta_u".to_string(),
            LogicParamValue::F64(10.0),
        );
        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.setpoint".to_string(), SignalValue::F64(10.0));
        inputs.insert("cv.y1.process_value".to_string(), SignalValue::F64(0.0));
        inputs.insert("mv.u1.hl".to_string(), SignalValue::F64(0.5));
        inputs.insert("mv.u1.ll".to_string(), SignalValue::F64(-0.5));
        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        let u = outputs
            .get("mv.u1.write")
            .and_then(SignalValue::as_f64)
            .expect("u");
        assert!((u - 0.5).abs() < 1.0e-3, "u={u}");
    }

    #[test]
    fn step_emits_canonical_future_prediction_logs() {
        let mut logic = one_by_one_logic();
        logic.params.insert(
            "mpc.prediction_horizon".to_string(),
            LogicParamValue::F64(3.0),
        );
        logic
            .params
            .insert("mpc.control_horizon".to_string(), LogicParamValue::F64(2.0));

        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.setpoint".to_string(), SignalValue::F64(2.0));
        inputs.insert("cv.y1.process_value".to_string(), SignalValue::F64(0.0));

        let _outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        let logs = logic.drain_user_logs();
        let values = &logs.first().expect("prediction log").values;

        assert!(values.contains_key("mpc.pred.free.y1.k01"));
        assert!(values.contains_key("mpc.pred.mpc.y1.k01"));
        assert!(values.contains_key("future_pred_without_control.cv.y1.k01"));
        assert!(values.contains_key("future_pred_with_control.cv.y1.k01"));
        assert!(values.contains_key("future_pred_with_control.mv.u1.k01"));
        assert!(values.contains_key("future_pred_with_control.mv.u1.k03"));
    }

    #[test]
    fn soft_band_produces_zero_move_inside_band() {
        let mut logic = one_by_one_logic();
        logic
            .params
            .insert("mpc.cv.y1.band_low".to_string(), LogicParamValue::F64(-1.0));
        logic
            .params
            .insert("mpc.cv.y1.band_high".to_string(), LogicParamValue::F64(1.0));
        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.setpoint".to_string(), SignalValue::F64(0.0));
        inputs.insert("cv.y1.process_value".to_string(), SignalValue::F64(0.2));
        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        let u = outputs
            .get("mv.u1.write")
            .and_then(SignalValue::as_f64)
            .expect("u");
        assert!(u.abs() < 1.0e-8);
    }

    #[test]
    fn soft_band_moves_toward_nearest_edge_outside_band() {
        let mut logic = one_by_one_logic();
        logic
            .params
            .insert("mpc.cv.y1.band_low".to_string(), LogicParamValue::F64(-0.1));
        logic
            .params
            .insert("mpc.cv.y1.band_high".to_string(), LogicParamValue::F64(0.1));
        logic
            .params
            .insert("mpc.mv.u1.r".to_string(), LogicParamValue::F64(1.0e-6));
        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.setpoint".to_string(), SignalValue::F64(1.0));
        inputs.insert("cv.y1.process_value".to_string(), SignalValue::F64(0.0));
        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        let u = outputs
            .get("mv.u1.write")
            .and_then(SignalValue::as_f64)
            .expect("u");
        assert!(u > 0.0, "u={u}");
    }

    #[test]
    fn external_cv_band_edge_overrides_tuning_and_uses_initial_fallback() {
        let mut logic = one_by_one_logic();
        logic
            .defaults
            .insert("mpc.cv.y1.band_low".to_string(), LogicParamValue::F64(-2.0));
        logic.params.insert(
            "mpc.cv.y1.band_low".to_string(),
            LogicParamValue::F64(-99.0),
        );
        logic
            .params
            .insert("mpc.cv.y1.band_high".to_string(), LogicParamValue::F64(3.0));
        let io = IoConfig {
            inputs: vec![crate::project_config::SignalPoint {
                name: "cv.y1.band_low".to_string(),
                signal_type: crate::types::SignalType::F64,
                address: "ns=2;s=BAND_LOW".to_string(),
                opc_tag: None,
            }],
            ..IoConfig::default()
        };
        let mut logic = logic.with_io_contract(&io);
        logic.set_params(HashMap::from([(
            "mpc.cv.y1.band_low".to_string(),
            LogicParamValue::F64(-88.0),
        )]));
        assert!(!logic.current_params().contains_key("mpc.cv.y1.band_low"));
        assert!(
            !logic
                .parameter_schema()
                .iter()
                .any(|entry| entry.name == "mpc.cv.y1.band_low")
        );
        assert!(logic.current_params().contains_key("mpc.cv.y1.band_high"));

        let mut inputs = HashMap::from([("cv.y1.band_low".to_string(), SignalValue::F64(-1.0))]);
        assert_eq!(logic.band_edges("y1", 100.0, &inputs), (99.0, 103.0));
        assert_eq!(logic.band_edges("y1", 110.0, &inputs), (109.0, 113.0));
        inputs.insert("cv.y1.band_low".to_string(), SignalValue::F64(f64::NAN));
        assert_eq!(logic.band_edges("y1", 100.0, &inputs), (98.0, 103.0));
        inputs.clear();
        assert_eq!(logic.band_edges("y1", 100.0, &inputs), (98.0, 103.0));
    }

    #[test]
    fn kalman_update_estimates_measured_state() {
        let mut logic = one_by_one_logic();
        let d = discretize_rk4(&logic.model, 1.0);
        logic.update_kalman(&[2.0], &DVector::from_vec(vec![0.0]), &d);
        assert!((logic.x_hat[0] - 2.0).abs() < 1.0e-6);
    }

    #[test]
    fn kalman_output_bias_learns_constant_cv_offset() {
        let mut logic = one_by_one_logic();
        logic.params.insert(
            "mpc.kalman.state.1.q".to_string(),
            LogicParamValue::F64(1.0e-8),
        );
        logic.params.insert(
            "mpc.kalman.cv.y1.bias_q".to_string(),
            LogicParamValue::F64(1.0e-1),
        );
        logic.params.insert(
            "mpc.kalman.cv.y1.r".to_string(),
            LogicParamValue::F64(1.0e-4),
        );
        let d = discretize_rk4(&logic.model, 1.0);

        for _ in 0..20 {
            logic.update_kalman(&[0.5], &DVector::from_vec(vec![0.0]), &d);
            logic.last_u_total = vec![0.0];
        }

        assert!(logic.y_bias[0] > 0.35, "bias={}", logic.y_bias[0]);
    }

    #[test]
    fn mpc_prediction_uses_estimated_cv_bias() {
        let mut logic = one_by_one_logic();
        logic.y_bias[0] = 0.5;
        logic.initialized = true;
        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.setpoint".to_string(), SignalValue::F64(0.5));
        inputs.insert("cv.y1.process_value".to_string(), SignalValue::F64(0.5));

        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        let u = outputs
            .get("mv.u1.write")
            .and_then(SignalValue::as_f64)
            .expect("u");
        assert!(u.abs() < 1.0e-8, "u={u}");
    }
}
