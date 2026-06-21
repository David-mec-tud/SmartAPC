use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, anyhow, bail};
use nalgebra::{DMatrix, DVector};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    project_config::{LogicParamValue, OnlineOptimizationConfig},
    types::{RuntimeState, SignalValue},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineOptimizationPhase {
    Running,
    Finished,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnlineOptimizationStatus {
    pub phase: OnlineOptimizationPhase,
    pub iteration: usize,
    pub max_iterations: usize,
    pub samples_in_window: u64,
    pub samples_per_window: u64,
    pub current_params: HashMap<String, f64>,
    pub best_params: HashMap<String, f64>,
    pub best_score: Option<f64>,
    pub safety_reason: Option<String>,
    pub finish_reason: Option<String>,
}

pub struct OnlineOptimizationTick {
    pub updates: Option<HashMap<String, LogicParamValue>>,
    pub events: Vec<Value>,
}

pub struct OnlineOptimizationSession {
    config: OnlineOptimizationConfig,
    original_params: HashMap<String, f64>,
    current_params: HashMap<String, f64>,
    current_unit: Vec<f64>,
    best_params: HashMap<String, f64>,
    best_score: f64,
    observed_x: Vec<Vec<f64>>,
    observed_y: Vec<f64>,
    samples_per_window: u64,
    samples_in_window: u64,
    score_sum: f64,
    delta_mv_integral: f64,
    previous_mvs: HashMap<String, f64>,
    dt_s: f64,
    no_improvement: usize,
    phase: OnlineOptimizationPhase,
    safety_reason: Option<String>,
    finish_reason: Option<String>,
    rng: DeterministicRng,
    stop_requested: Arc<AtomicBool>,
}

impl OnlineOptimizationSession {
    pub fn new(
        config: OnlineOptimizationConfig,
        current: &HashMap<String, LogicParamValue>,
        logic_cycle_ms: u64,
        stop_requested: Arc<AtomicBool>,
    ) -> Result<(Self, Vec<Value>)> {
        if config.parameters.is_empty() {
            bail!("select at least one parameter for online optimization");
        }
        let mut original_params = HashMap::new();
        for row in &config.parameters {
            let value = current
                .get(&row.key)
                .and_then(|value| match value {
                    LogicParamValue::F64(value) => Some(*value),
                    LogicParamValue::Bool(_) => None,
                })
                .ok_or_else(|| {
                    anyhow!("online optimization parameter '{}' is not numeric", row.key)
                })?;
            original_params.insert(row.key.clone(), value);
        }
        let current_unit = unit_from_candidate(&config, &original_params);
        let samples_per_window = ((config.duration_s.saturating_mul(1_000) + logic_cycle_ms - 1)
            / logic_cycle_ms)
            .max(1);
        let session = Self {
            best_params: original_params.clone(),
            current_params: original_params.clone(),
            original_params,
            current_unit,
            best_score: f64::INFINITY,
            observed_x: Vec::new(),
            observed_y: Vec::new(),
            samples_per_window,
            samples_in_window: 0,
            score_sum: 0.0,
            delta_mv_integral: 0.0,
            previous_mvs: HashMap::new(),
            dt_s: logic_cycle_ms as f64 / 1_000.0,
            no_improvement: 0,
            phase: OnlineOptimizationPhase::Running,
            safety_reason: None,
            finish_reason: None,
            rng: DeterministicRng::new(0x0A11_1E0F_71A1_2026),
            stop_requested,
            config,
        };
        let events = vec![
            json!({"event":"online_optimization_start","params":session.current_params}),
            session.candidate_start_event(),
        ];
        Ok((session, events))
    }

    pub fn status(&self) -> OnlineOptimizationStatus {
        OnlineOptimizationStatus {
            phase: self.phase,
            iteration: self.observed_y.len(),
            max_iterations: self.config.max_iterations,
            samples_in_window: self.samples_in_window,
            samples_per_window: self.samples_per_window,
            current_params: self.current_params.clone(),
            best_params: self.best_params.clone(),
            best_score: self.best_score.is_finite().then_some(self.best_score),
            safety_reason: self.safety_reason.clone(),
            finish_reason: self.finish_reason.clone(),
        }
    }

    pub fn pre_cycle(
        &mut self,
        state: RuntimeState,
        inputs: &HashMap<String, SignalValue>,
    ) -> OnlineOptimizationTick {
        if self.phase != OnlineOptimizationPhase::Running {
            return empty_tick();
        }
        if self.stop_requested.swap(false, Ordering::AcqRel) {
            return self.finish_with_original("stopped_by_user".to_string());
        }
        if state != RuntimeState::Running {
            return self.finish_with_original(format!("execution entered state {state:?}"));
        }

        for cv in &self.config.cvs {
            let process_value = inputs
                .get(&format!("cv.{}.process_value", cv.name))
                .and_then(SignalValue::as_f64);
            let Some(process_value) = process_value else {
                return self
                    .finish_with_original(format!("missing numeric CV data for '{}'", cv.name));
            };
            if !process_value.is_finite() {
                return self.finish_with_original(format!("non-finite CV data for '{}'", cv.name));
            }
            if process_value <= cv.low || process_value >= cv.high {
                let reason = format!(
                    "CV '{}' touched optimization safety limit [{}, {}] at {}",
                    cv.name, cv.low, cv.high, process_value
                );
                self.safety_reason = Some(reason.clone());
                let mut tick = self.finish_with_original(reason.clone());
                tick.events.insert(
                    0,
                    json!({"event":"online_optimization_safety_stop","reason":reason,"candidate":self.current_params}),
                );
                return tick;
            }
        }
        empty_tick()
    }

    pub fn observe(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        outputs: &HashMap<String, SignalValue>,
    ) -> OnlineOptimizationTick {
        if self.phase != OnlineOptimizationPhase::Running {
            return empty_tick();
        }
        let mut sample_score = 0.0;
        for cv in &self.config.cvs {
            let setpoint = inputs
                .get(&format!("cv.{}.setpoint", cv.name))
                .and_then(SignalValue::as_f64);
            let process_value = inputs
                .get(&format!("cv.{}.process_value", cv.name))
                .and_then(SignalValue::as_f64);
            let (Some(setpoint), Some(process_value)) = (setpoint, process_value) else {
                return self
                    .finish_with_original(format!("missing numeric CV data for '{}'", cv.name));
            };
            if !setpoint.is_finite() || !process_value.is_finite() {
                return self.finish_with_original(format!("non-finite CV data for '{}'", cv.name));
            }
            sample_score += cv.weight * (setpoint - process_value).powi(2);
        }
        for mv in &self.config.delta_mvs {
            let value = outputs
                .get(&format!("mv.{}.write", mv.name))
                .and_then(SignalValue::as_f64);
            let Some(value) = value else {
                return self.finish_with_original(format!(
                    "missing numeric MV output data for '{}'",
                    mv.name
                ));
            };
            if !value.is_finite() {
                return self
                    .finish_with_original(format!("non-finite MV output data for '{}'", mv.name));
            }
            if let Some(previous) = self.previous_mvs.insert(mv.name.clone(), value) {
                self.delta_mv_integral += mv.weight * (value - previous).powi(2) * self.dt_s;
            }
        }
        self.score_sum += sample_score;
        self.samples_in_window += 1;
        if self.samples_in_window < self.samples_per_window {
            return empty_tick();
        }
        self.complete_candidate(
            self.score_sum / self.samples_in_window as f64 + self.delta_mv_integral,
        )
    }

    pub fn stop(&mut self, reason: &str) -> OnlineOptimizationTick {
        self.finish_with_original(reason.to_string())
    }

    fn complete_candidate(&mut self, score: f64) -> OnlineOptimizationTick {
        self.observed_x.push(self.current_unit.clone());
        self.observed_y.push(score);
        let mut events = vec![json!({
            "event":"online_optimization_candidate_complete",
            "iteration":self.observed_y.len(),
            "score":score,
            "candidate":self.current_params
        })];
        if score + 1.0e-12 < self.best_score {
            self.best_score = score;
            self.best_params = self.current_params.clone();
            self.no_improvement = 0;
        } else {
            self.no_improvement += 1;
        }
        let converged = self.observed_y.len() >= self.config.initial_samples
            && self.no_improvement >= self.config.convergence_patience;
        if self.stop_requested.swap(false, Ordering::AcqRel) {
            let mut tick = self.finish_with_original("stopped_by_user".to_string());
            events.append(&mut tick.events);
            tick.events = events;
            return tick;
        }
        if self.observed_y.len() >= self.config.max_iterations || converged {
            let reason = if converged {
                "converged"
            } else {
                "max_iterations"
            };
            let mut tick = self.finish_with_best(reason.to_string());
            events.append(&mut tick.events);
            tick.events = events;
            return tick;
        }
        self.current_unit = if self.observed_y.len() < self.config.initial_samples {
            latin_like_sample(
                self.observed_y.len(),
                self.config.initial_samples,
                self.config.parameters.len(),
                &mut self.rng,
            )
        } else {
            propose_expected_improvement(
                &self.observed_x,
                &self.observed_y,
                self.config.parameters.len(),
                &mut self.rng,
                &self.stop_requested,
            )
        };
        if self.stop_requested.swap(false, Ordering::AcqRel) {
            let mut tick = self.finish_with_original("stopped_by_user".to_string());
            events.append(&mut tick.events);
            tick.events = events;
            return tick;
        }
        self.current_params = candidate_from_unit(&self.config, &self.current_unit);
        self.phase = OnlineOptimizationPhase::Running;
        self.safety_reason = None;
        self.samples_in_window = 0;
        self.score_sum = 0.0;
        self.delta_mv_integral = 0.0;
        self.previous_mvs.clear();
        events.push(self.candidate_start_event());
        OnlineOptimizationTick {
            updates: Some(to_logic_params(&self.current_params)),
            events,
        }
    }

    fn finish_with_best(&mut self, reason: String) -> OnlineOptimizationTick {
        self.phase = OnlineOptimizationPhase::Finished;
        self.finish_reason = Some(reason.clone());
        let restore = self.best_or_original_params();
        self.current_params = restore.clone();
        OnlineOptimizationTick {
            updates: Some(to_logic_params(&restore)),
            events: vec![
                self.restore_event(&reason, &restore),
                json!({"event":"online_optimization_finish","reason":reason,"best_score":self.best_score.is_finite().then_some(self.best_score),"best_params":restore}),
            ],
        }
    }

    fn finish_with_original(&mut self, reason: String) -> OnlineOptimizationTick {
        self.phase = OnlineOptimizationPhase::Finished;
        self.finish_reason = Some(reason.clone());
        let restore = self.original_params.clone();
        self.current_params = restore.clone();
        OnlineOptimizationTick {
            updates: Some(to_logic_params(&restore)),
            events: vec![
                self.restore_event(&reason, &restore),
                json!({"event":"online_optimization_finish","reason":reason,"best_score":self.best_score.is_finite().then_some(self.best_score),"best_params":self.best_params,"restored_params":restore}),
            ],
        }
    }

    fn best_or_original_params(&self) -> HashMap<String, f64> {
        if self.best_score.is_finite() {
            self.best_params.clone()
        } else {
            self.original_params.clone()
        }
    }

    fn candidate_start_event(&self) -> Value {
        json!({"event":"online_optimization_candidate_start","iteration":self.observed_y.len() + 1,"candidate":self.current_params})
    }

    fn restore_event(&self, reason: &str, params: &HashMap<String, f64>) -> Value {
        json!({"event":"online_optimization_parameters_restored","reason":reason,"params":params})
    }
}

fn empty_tick() -> OnlineOptimizationTick {
    OnlineOptimizationTick {
        updates: None,
        events: Vec::new(),
    }
}

fn to_logic_params(values: &HashMap<String, f64>) -> HashMap<String, LogicParamValue> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), LogicParamValue::F64(*value)))
        .collect()
}

fn unit_from_candidate(
    config: &OnlineOptimizationConfig,
    values: &HashMap<String, f64>,
) -> Vec<f64> {
    config
        .parameters
        .iter()
        .map(|row| {
            ((values.get(&row.key).copied().unwrap_or(row.min) - row.min) / (row.max - row.min))
                .clamp(0.0, 1.0)
        })
        .collect()
}

fn candidate_from_unit(config: &OnlineOptimizationConfig, unit: &[f64]) -> HashMap<String, f64> {
    config
        .parameters
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let value = row.min + unit.get(index).copied().unwrap_or(0.5) * (row.max - row.min);
            let value = if row.key.ends_with("prediction_horizon")
                || row.key.ends_with("control_horizon")
            {
                value.round().max(1.0)
            } else {
                value
            };
            (row.key.clone(), value)
        })
        .collect()
}

#[derive(Debug, Clone)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 as f64) / (u64::MAX as f64)
    }
}

fn latin_like_sample(
    iteration: usize,
    sample_count: usize,
    dim: usize,
    rng: &mut DeterministicRng,
) -> Vec<f64> {
    (0..dim)
        .map(|axis| {
            let shuffled = (iteration * 37 + axis * 17) % sample_count.max(1);
            ((shuffled as f64) + rng.next_unit()) / sample_count.max(1) as f64
        })
        .collect()
}

fn propose_expected_improvement(
    xs: &[Vec<f64>],
    ys: &[f64],
    dim: usize,
    rng: &mut DeterministicRng,
    stop_requested: &AtomicBool,
) -> Vec<f64> {
    if xs.len() < 2 || dim == 0 {
        return (0..dim).map(|_| rng.next_unit()).collect();
    }
    let best = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let y_mean = ys.iter().sum::<f64>() / ys.len() as f64;
    let y_scale = ys
        .iter()
        .map(|value| (value - y_mean).abs())
        .fold(1.0_f64, f64::max);
    let y_vec = DVector::from_iterator(ys.len(), ys.iter().map(|value| (value - y_mean) / y_scale));
    let mut k = DMatrix::zeros(xs.len(), xs.len());
    for row in 0..xs.len() {
        for col in 0..xs.len() {
            k[(row, col)] = rbf_kernel(&xs[row], &xs[col]);
        }
        k[(row, row)] += 1.0e-6;
    }
    let Some(k_inv) = k.try_inverse() else {
        return (0..dim).map(|_| rng.next_unit()).collect();
    };
    let alpha = &k_inv * y_vec;
    let mut best_x = (0..dim).map(|_| rng.next_unit()).collect::<Vec<_>>();
    let mut best_ei = f64::NEG_INFINITY;
    for _ in 0..(256usize).max(dim * 64) {
        if stop_requested.load(Ordering::Acquire) {
            break;
        }
        let x = (0..dim).map(|_| rng.next_unit()).collect::<Vec<_>>();
        let k_star = DVector::from_iterator(xs.len(), xs.iter().map(|seen| rbf_kernel(&x, seen)));
        let mu = k_star.dot(&alpha) * y_scale + y_mean;
        let variance = (1.0 - (k_star.transpose() * &k_inv * &k_star)[(0, 0)]).max(1.0e-12);
        let sigma = variance.sqrt() * y_scale;
        let improvement = best - mu;
        let z = improvement / sigma;
        let ei = improvement * normal_cdf(z) + sigma * normal_pdf(z);
        if ei.is_finite() && ei > best_ei {
            best_ei = ei;
            best_x = x;
        }
    }
    best_x
}

fn rbf_kernel(left: &[f64], right: &[f64]) -> f64 {
    let dist2: f64 = left
        .iter()
        .zip(right)
        .map(|(left, right)| (left - right).powi(2))
        .sum();
    (-dist2 / (2.0 * 0.35_f64.powi(2))).exp()
}

fn normal_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf_approx(x / 2.0_f64.sqrt()))
}

fn erf_approx(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    sign * y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_config::{
        OnlineOptimizationCvConfig, OnlineOptimizationDeltaMvConfig, OnlineOptimizationParamConfig,
    };

    fn config() -> OnlineOptimizationConfig {
        OnlineOptimizationConfig {
            parameters: vec![OnlineOptimizationParamConfig {
                key: "kp".into(),
                min: 0.0,
                max: 10.0,
            }],
            cvs: vec![OnlineOptimizationCvConfig {
                name: "y".into(),
                weight: 1.0,
                low: -10.0,
                high: 10.0,
            }],
            delta_mvs: Vec::new(),
            duration_s: 1,
            max_iterations: 2,
            initial_samples: 1,
            convergence_patience: 10,
        }
    }

    #[test]
    fn first_candidate_uses_current_value_and_scores_mse() {
        let current = HashMap::from([("kp".into(), LogicParamValue::F64(3.0))]);
        let (mut session, _) = OnlineOptimizationSession::new(
            config(),
            &current,
            1_000,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(session.status().current_params["kp"], 3.0);
        let inputs = HashMap::from([
            ("cv.y.setpoint".into(), SignalValue::F64(2.0)),
            ("cv.y.process_value".into(), SignalValue::F64(1.0)),
        ]);
        session.pre_cycle(RuntimeState::Running, &inputs);
        session.observe(&inputs, &HashMap::new());
        assert_eq!(session.status().best_score, Some(1.0));
    }

    #[test]
    fn safety_touch_stops_and_restores_original() {
        let current = HashMap::from([("kp".into(), LogicParamValue::F64(3.0))]);
        let (mut session, _) = OnlineOptimizationSession::new(
            config(),
            &current,
            1_000,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let inputs = HashMap::from([
            ("cv.y.setpoint".into(), SignalValue::F64(2.0)),
            ("cv.y.process_value".into(), SignalValue::F64(10.0)),
        ]);
        let tick = session.pre_cycle(RuntimeState::Running, &inputs);
        assert_eq!(session.status().phase, OnlineOptimizationPhase::Finished);
        assert_eq!(tick.updates.unwrap()["kp"], LogicParamValue::F64(3.0));
        assert!(
            tick.events
                .iter()
                .any(|event| { event["event"] == "online_optimization_safety_stop" })
        );
    }

    #[test]
    fn stop_request_restores_original_before_observation() {
        let current = HashMap::from([("kp".into(), LogicParamValue::F64(3.0))]);
        let stop = Arc::new(AtomicBool::new(false));
        let (mut session, _) =
            OnlineOptimizationSession::new(config(), &current, 1_000, Arc::clone(&stop)).unwrap();
        stop.store(true, Ordering::Release);
        let stopped = session.pre_cycle(RuntimeState::Running, &HashMap::new());
        assert_eq!(session.status().phase, OnlineOptimizationPhase::Finished);
        assert_eq!(stopped.updates.unwrap()["kp"], LogicParamValue::F64(3.0));
    }

    #[test]
    fn delta_mv_term_integrates_squared_moves() {
        let mut config = config();
        config.cvs.clear();
        config.delta_mvs = vec![OnlineOptimizationDeltaMvConfig {
            name: "u".into(),
            weight: 2.0,
        }];
        config.duration_s = 2;
        let current = HashMap::from([("kp".into(), LogicParamValue::F64(3.0))]);
        let (mut session, _) = OnlineOptimizationSession::new(
            config,
            &current,
            1_000,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let first = HashMap::from([("mv.u.write".into(), SignalValue::F64(1.0))]);
        let second = HashMap::from([("mv.u.write".into(), SignalValue::F64(4.0))]);
        session.observe(&HashMap::new(), &first);
        session.observe(&HashMap::new(), &second);
        assert_eq!(session.status().best_score, Some(18.0));
    }
}
