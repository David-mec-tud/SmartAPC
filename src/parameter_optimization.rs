use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use anyhow::{Context, Result, anyhow, bail};
use nalgebra::{DMatrix, DVector};

use crate::{
    logic::ControlLogic,
    project::ProjectService,
    project_config::{
        IoConfig, LimitsConfig, LinearStateSpaceModelConfig, LogicKind, LogicParamValue,
        ModelConfig, ParameterOptimizationConfig, ProjectConfig, SimulationConfig,
        VariableCatalogConfig,
    },
    runtime::build_logic,
    simulation::derive_initial_cv_offsets,
    types::{RuntimeState, SignalValue},
};

const LARGE_PENALTY: f64 = 1.0e18;
const BO_SEED: u64 = 0x5A17_0A9C_D15C_A11E;

#[derive(Debug, Clone)]
pub struct OptimizationProgress {
    pub iteration: usize,
    pub max_iterations: usize,
    pub best_score: Option<f64>,
    pub best_mean_score: Option<f64>,
    pub convergence_history: Vec<(usize, f64)>,
    pub converged: bool,
    pub cancelled: bool,
    pub status: String,
    pub warnings: Vec<String>,
    pub best_params: HashMap<String, f64>,
    pub curves: Option<OptimizationCurveFamily>,
}

#[derive(Debug, Clone)]
pub struct OptimizationReport {
    pub best_score: f64,
    pub best_mean_score: f64,
    pub best_params: HashMap<String, f64>,
    pub iteration_count: usize,
    pub converged: bool,
    pub cancelled: bool,
    pub warnings: Vec<String>,
    pub curves: OptimizationCurveFamily,
    pub convergence_history: Vec<(usize, f64)>,
}

#[derive(Debug, Clone)]
pub enum OptimizationEvent {
    Progress(OptimizationProgress),
    Finished(Result<OptimizationReport, String>),
}

#[derive(Debug, Clone, Default)]
pub struct OptimizationCurveFamily {
    pub cvs: Vec<CvCurveFamily>,
    pub inputs: Vec<InputCurveFamily>,
}

#[derive(Debug, Clone)]
pub struct CvCurveFamily {
    pub cv: String,
    pub target: f64,
    pub scenarios: Vec<StepResponseCurve>,
}

#[derive(Debug, Clone)]
pub struct InputCurveFamily {
    pub input: String,
    pub scenarios: Vec<StepResponseCurve>,
}

#[derive(Debug, Clone)]
pub struct StepResponseCurve {
    pub scenario_index: usize,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StepMetrics {
    pub ise: f64,
    pub iae: f64,
    pub itae: f64,
    pub overshoot: f64,
}

#[derive(Debug, Clone)]
struct OptimizableParam {
    key: String,
    start: f64,
    min: f64,
    max: f64,
    fixed: bool,
}

#[derive(Debug, Clone)]
struct Scenario {
    a_scale_pct: Vec<Vec<f64>>,
    b_scale_pct: Vec<Vec<f64>>,
    noise_seed: u64,
}

#[derive(Debug, Clone)]
struct CandidateResult {
    score: f64,
    curves: OptimizationCurveFamily,
    warnings: Vec<String>,
}

pub fn run_parameter_optimization(
    project_service: &ProjectService,
    project_name: &str,
    cancel: Arc<AtomicBool>,
    progress_tx: Option<Sender<OptimizationEvent>>,
) -> Result<OptimizationReport> {
    let mut config = project_service.load_config(project_name)?;
    config.normalize_model_config();
    config.normalize_simulation_config();
    config.sync_active_online_params();
    config.normalize_optimization_config();
    config.validate_for_simulation()?;

    if config.logic.kind == LogicKind::CustomRust {
        bail!("parameter optimization supports P-controller and Linear MPC only");
    }

    let model = config
        .model
        .as_ref()
        .and_then(ModelConfig::linear_state_space)
        .cloned()
        .ok_or_else(|| anyhow!("parameter optimization requires a linear state-space model"))?;
    let (logic_io, limits) = config.build_simulation_io_and_limits()?;
    let project_dir = project_service.project_dir(project_name);
    let params = optimization_params(&config.optimization)?;
    let scenarios = sample_scenarios(&model, &config.optimization);

    let mut warnings = Vec::new();
    let variable_count = params.iter().filter(|param| !param.fixed).count();
    if params.is_empty() {
        bail!("no active numeric controller parameters are available for optimization");
    }

    let max_iterations = if variable_count == 0 {
        1
    } else {
        config.optimization.max_iterations
    };
    let initial_samples = config.optimization.initial_samples.min(max_iterations);
    let mut rng = DeterministicRng::new(BO_SEED);
    let mut observed_x: Vec<Vec<f64>> = Vec::new();
    let mut observed_y: Vec<f64> = Vec::new();
    let mut best_score = f64::INFINITY;
    let mut best_params = candidate_from_unit(&params, &vec![0.5; variable_count]);
    let mut best_curves = OptimizationCurveFamily::default();
    let mut no_improvement = 0usize;
    let mut converged = false;
    let mut completed_iterations = 0usize;
    let objective_normalizer = optimization_objective_normalizer(&config.optimization);
    let mut convergence_history = Vec::new();

    for iteration in 0..max_iterations {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let unit = if variable_count == 0 {
            Vec::new()
        } else if iteration < initial_samples {
            latin_like_sample(iteration, initial_samples, variable_count, &mut rng)
        } else {
            propose_expected_improvement(&observed_x, &observed_y, variable_count, &mut rng)
        };
        let candidate = candidate_from_unit(&params, &unit);
        let result = evaluate_candidate(
            &config,
            &model,
            &logic_io,
            &limits,
            &project_dir,
            &scenarios,
            &candidate,
        )
        .unwrap_or_else(|err| CandidateResult {
            score: LARGE_PENALTY,
            curves: best_curves.clone(),
            warnings: vec![format!("candidate failed: {err}")],
        });

        warnings.extend(result.warnings.clone());
        if variable_count > 0 {
            observed_x.push(unit);
            observed_y.push(result.score);
        }
        completed_iterations = iteration + 1;

        if result.score + 1.0e-9 < best_score {
            best_score = result.score;
            best_params = candidate;
            best_curves = result.curves;
            no_improvement = 0;
        } else {
            no_improvement += 1;
        }
        let best_mean_score = best_score / objective_normalizer;
        if best_mean_score.is_finite() {
            convergence_history.push((completed_iterations, best_mean_score));
        }

        let should_send_curves = completed_iterations == 1
            || completed_iterations % config.optimization.plot_interval == 0
            || completed_iterations == max_iterations;
        send_progress(
            &progress_tx,
            OptimizationProgress {
                iteration: completed_iterations,
                max_iterations,
                best_score: best_score.is_finite().then_some(best_score),
                best_mean_score: best_mean_score.is_finite().then_some(best_mean_score),
                convergence_history: convergence_history.clone(),
                converged,
                cancelled: false,
                status: format!("iteration {} / {}", completed_iterations, max_iterations),
                warnings: tail_warnings(&warnings),
                best_params: best_params.clone(),
                curves: should_send_curves.then_some(best_curves.clone()),
            },
        );

        if variable_count > 0
            && completed_iterations >= initial_samples
            && no_improvement >= config.optimization.convergence_patience
        {
            converged = true;
            break;
        }
    }

    let cancelled = cancel.load(Ordering::Relaxed);
    if best_curves.cvs.is_empty() {
        let result = evaluate_candidate(
            &config,
            &model,
            &logic_io,
            &limits,
            &project_dir,
            &scenarios,
            &best_params,
        )?;
        best_score = result.score;
        best_curves = result.curves;
    }
    let best_mean_score = best_score / objective_normalizer;

    send_progress(
        &progress_tx,
        OptimizationProgress {
            iteration: completed_iterations,
            max_iterations,
            best_score: best_score.is_finite().then_some(best_score),
            best_mean_score: best_mean_score.is_finite().then_some(best_mean_score),
            convergence_history: convergence_history.clone(),
            converged,
            cancelled,
            status: if cancelled {
                "optimization cancelled".to_string()
            } else if converged {
                "optimization converged".to_string()
            } else {
                "optimization finished".to_string()
            },
            warnings: tail_warnings(&warnings),
            best_params: best_params.clone(),
            curves: Some(best_curves.clone()),
        },
    );

    Ok(OptimizationReport {
        best_score,
        best_mean_score,
        best_params,
        iteration_count: completed_iterations,
        converged,
        cancelled,
        warnings: tail_warnings(&warnings),
        curves: best_curves,
        convergence_history,
    })
}

pub fn apply_optimized_parameters(
    project_service: &ProjectService,
    project_name: &str,
    updates: &HashMap<String, f64>,
) -> Result<ProjectConfig> {
    let mut cfg = project_service.load_config(project_name)?;
    cfg.sync_active_online_params();

    for (key, value) in updates {
        cfg.logic.params.insert(
            key.clone(),
            LogicParamValue::F64(rounded_controller_value(key, *value)),
        );

        if cfg.logic.kind == LogicKind::PController {
            for pair in &mut cfg.logic.p_controller.pairs {
                let kp_key = format!("p.{}.{}.kp", pair.cv, pair.mv);
                let bias_key = format!("p.{}.{}.bias", pair.cv, pair.mv);
                if key == &kp_key {
                    pair.kp = *value;
                } else if key == &bias_key {
                    pair.bias = *value;
                }
            }
        }
    }

    cfg.sync_active_online_params();
    project_service.save_config(project_name, &cfg)?;
    project_service.load_config(project_name)
}

fn optimization_params(config: &ParameterOptimizationConfig) -> Result<Vec<OptimizableParam>> {
    let mut out = Vec::new();
    for row in &config.parameters {
        let start = rounded_controller_value(&row.key, row.start);
        let min = row.min.min(row.max);
        let max = row.min.max(row.max);
        if !row.fixed && (max - min).abs() <= f64::EPSILON {
            bail!("optimization range for '{}' has no width", row.key);
        }
        out.push(OptimizableParam {
            key: row.key.clone(),
            start,
            min,
            max,
            fixed: row.fixed,
        });
    }
    Ok(out)
}

fn evaluate_candidate(
    config: &ProjectConfig,
    model: &LinearStateSpaceModelConfig,
    logic_io: &IoConfig,
    limits: &LimitsConfig,
    project_dir: &Path,
    scenarios: &[Scenario],
    candidate: &HashMap<String, f64>,
) -> Result<CandidateResult> {
    let mut worst_score = f64::NEG_INFINITY;
    let mut family = OptimizationCurveFamily {
        cvs: config
            .variables
            .cvs
            .iter()
            .map(|cv| CvCurveFamily {
                cv: cv.clone(),
                target: *config.optimization.cv_target_steps.get(cv).unwrap_or(&0.0),
                scenarios: Vec::new(),
            })
            .collect(),
        inputs: model
            .u_order
            .iter()
            .map(|input| InputCurveFamily {
                input: input.clone(),
                scenarios: Vec::new(),
            })
            .collect(),
    };
    let mut warnings = Vec::new();

    for (scenario_index, scenario) in scenarios.iter().enumerate() {
        let mut scenario_config = config.clone();
        for (key, value) in candidate {
            scenario_config.logic.params.insert(
                key.clone(),
                LogicParamValue::F64(rounded_controller_value(key, *value)),
            );
        }
        scenario_config.simulation.cv_setpoints =
            scenario_config.optimization.cv_target_steps.clone();
        scenario_config.simulation.cv_noise_std = scenario_config.optimization.cv_noise_std.clone();
        scenario_config.simulation.mv_noise_std = scenario_config.optimization.mv_noise_std.clone();
        scenario_config.simulation.a_scale_pct = scenario.a_scale_pct.clone();
        scenario_config.simulation.b_scale_pct = scenario.b_scale_pct.clone();
        scenario_config.sync_active_online_params();

        let logic = build_logic(&scenario_config, logic_io, project_dir)?;
        let response = run_step_response(
            &scenario_config,
            model,
            logic_io,
            limits,
            logic,
            scenario.noise_seed,
        )?;
        let scenario_score = weighted_score(
            &scenario_config.optimization,
            &response.metrics,
            response.actuator,
        );
        if !scenario_score.is_finite() {
            warnings.push(format!(
                "scenario {} produced a non-finite score",
                scenario_index + 1
            ));
        }
        if scenario_score > worst_score {
            worst_score = scenario_score;
        }
        for cv_family in &mut family.cvs {
            if let Some(points) = response.curves.get(&cv_family.cv) {
                cv_family.scenarios.push(StepResponseCurve {
                    scenario_index,
                    points: points.clone(),
                });
            }
        }
        for input_family in &mut family.inputs {
            if let Some(points) = response.input_curves.get(&input_family.input) {
                input_family.scenarios.push(StepResponseCurve {
                    scenario_index,
                    points: points.clone(),
                });
            }
        }
    }

    Ok(CandidateResult {
        score: if worst_score.is_finite() {
            worst_score
        } else {
            LARGE_PENALTY
        },
        curves: family,
        warnings,
    })
}

#[derive(Debug, Clone)]
struct StepResponseResult {
    metrics: HashMap<String, StepMetrics>,
    curves: HashMap<String, Vec<(f64, f64)>>,
    input_curves: HashMap<String, Vec<(f64, f64)>>,
    actuator: f64,
}

fn run_step_response(
    config: &ProjectConfig,
    model: &LinearStateSpaceModelConfig,
    logic_io: &IoConfig,
    limits: &LimitsConfig,
    mut logic: Box<dyn ControlLogic>,
    noise_seed: u64,
) -> Result<StepResponseResult> {
    let dt_s = config.runtime.cycle_ms as f64 / 1000.0;
    let steps = (config.optimization.simulation_duration_s / dt_s)
        .ceil()
        .max(1.0) as usize;
    let mut simulation = config.simulation.clone();
    simulation.cv_offsets = derive_initial_cv_offsets(model, &simulation, &config.variables)?;
    let (a_scaled, b_scaled) = scaled_model_matrices(model, &simulation);
    let mut x = solve_state_steady_state(
        &a_scaled,
        &b_scaled,
        &initial_u_vector(model, &simulation, &config.variables),
    )?;
    let mut rng = DeterministicRng::new(noise_seed);

    logic.init(
        config.logic.initial_values.clone(),
        config.logic.params.clone(),
    );
    let mut current_cv = simulation.cv_initials.clone();
    let mut current_mv = simulation.mv_initials.clone();
    let mut current_dv = simulation.dv_initials.clone();
    let mut applied_mv = simulation.mv_initials.clone();
    let mut metrics: HashMap<String, StepMetrics> = HashMap::new();
    let mut actuator = 0.0;
    let mut curves: HashMap<String, Vec<(f64, f64)>> = config
        .variables
        .cvs
        .iter()
        .map(|cv| (cv.clone(), Vec::with_capacity(steps + 1)))
        .collect();
    let mut input_curves: HashMap<String, Vec<(f64, f64)>> = model
        .u_order
        .iter()
        .map(|input| (input.clone(), Vec::with_capacity(steps + 1)))
        .collect();

    for step in 0..=steps {
        let t = step as f64 * dt_s;
        for cv in &config.variables.cvs {
            let value = *current_cv.get(cv).unwrap_or(&0.0);
            if let Some(points) = curves.get_mut(cv) {
                points.push((t, value));
            }
            let target = *simulation.cv_setpoints.get(cv).unwrap_or(&0.0);
            let initial = *simulation.cv_initials.get(cv).unwrap_or(&0.0);
            let metric = metrics.entry(cv.clone()).or_default();
            accumulate_metric(metric, target - value, t, dt_s);
            accumulate_overshoot_metric(metric, value, initial, target, dt_s);
        }
        for input in &model.u_order {
            if let Some(points) = input_curves.get_mut(input) {
                let value = applied_mv
                    .get(input)
                    .copied()
                    .unwrap_or_else(|| current_dv.get(input).copied().unwrap_or(0.0));
                points.push((t, value));
            }
        }

        if step == steps {
            break;
        }

        let inputs = build_inputs(
            logic_io,
            config,
            &simulation,
            &current_cv,
            &current_mv,
            &current_dv,
            &logic.current_params(),
        );
        let mut outputs = logic.step(&inputs, RuntimeState::Running)?;
        apply_output_limits(limits, &mut outputs);

        for mv in &config.variables.mvs {
            let key = format!("mv.{mv}.write");
            if let Some(value) = outputs.get(&key).and_then(SignalValue::as_f64) {
                let previous = applied_mv.get(mv).copied().unwrap_or(0.0);
                let delta = value - previous;
                // Match the Linear MPC R term: penalize actuator movement, not noisy readback.
                actuator += delta * delta * dt_s;
                applied_mv.insert(mv.clone(), value);
            }
        }

        let u = current_u_vector(model, &applied_mv, &current_dv);
        x = rk4_step(&a_scaled, &b_scaled, &x, &u, dt_s);
        let y = output_vector(model, &x, &u);

        for (index, cv) in model.y_order.iter().enumerate() {
            let offset = simulation.cv_offsets.get(cv).copied().unwrap_or(0.0);
            let noise = rng.gaussian(0.0, simulation.cv_noise_std.get(cv).copied().unwrap_or(0.0));
            current_cv.insert(cv.clone(), y[index] + offset + noise);
        }
        for mv in &config.variables.mvs {
            let base = applied_mv.get(mv).copied().unwrap_or(0.0);
            let noise = rng.gaussian(0.0, simulation.mv_noise_std.get(mv).copied().unwrap_or(0.0));
            let mut value = base + noise;
            if let Some(limit) = limits.values.get(&format!("mv.{mv}.write")) {
                value = value.clamp(limit.min, limit.max);
            }
            current_mv.insert(mv.clone(), value);
        }
        for dv in &config.variables.dvs {
            current_dv.insert(
                dv.clone(),
                simulation.dv_disturbances.get(dv).copied().unwrap_or(0.0),
            );
        }
    }

    Ok(StepResponseResult {
        metrics,
        curves,
        input_curves,
        actuator,
    })
}

pub fn accumulate_metric(metrics: &mut StepMetrics, error: f64, t: f64, dt: f64) {
    let abs = error.abs();
    metrics.ise += error * error * dt;
    metrics.iae += abs * dt;
    metrics.itae += t * abs * dt;
}

fn accumulate_overshoot_metric(
    metrics: &mut StepMetrics,
    value: f64,
    initial: f64,
    target: f64,
    dt: f64,
) {
    let step_delta = target - initial;
    // Overshoot is measured only beyond the final target in the direction of the requested step.
    let overshoot = if step_delta > f64::EPSILON {
        (value - target).max(0.0)
    } else if step_delta < -f64::EPSILON {
        (target - value).max(0.0)
    } else {
        (value - target).abs()
    };
    metrics.overshoot += overshoot * overshoot * dt;
}

pub fn robust_worst_score(scores: &[f64]) -> f64 {
    scores.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

fn weighted_score(
    config: &ParameterOptimizationConfig,
    metrics: &HashMap<String, StepMetrics>,
    actuator: f64,
) -> f64 {
    let mut score = 0.0;
    for (cv, metric) in metrics {
        let channel_weight = config.cv_weights.get(cv).copied().unwrap_or(1.0);
        score += channel_weight
            * (config.metric_weights.ise * metric.ise
                + config.metric_weights.iae * metric.iae
                + config.metric_weights.itae * metric.itae
                + config.metric_weights.overshoot * metric.overshoot);
    }
    score += config.metric_weights.actuator * actuator;
    score
}

fn optimization_objective_normalizer(config: &ParameterOptimizationConfig) -> f64 {
    let total_weight = config
        .cv_weights
        .values()
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
        .sum::<f64>();
    total_weight.max(1.0)
}

fn sample_scenarios(
    model: &LinearStateSpaceModelConfig,
    config: &ParameterOptimizationConfig,
) -> Vec<Scenario> {
    let mut rng = DeterministicRng::new(0xA5A5_2026_F00D_1234);
    let mut scenarios = Vec::with_capacity(config.scenario_count);
    for _ in 0..config.scenario_count {
        scenarios.push(Scenario {
            a_scale_pct: sample_scale_matrix(
                &model.a,
                config.a_scale_min_pct,
                config.a_scale_max_pct,
                &mut rng,
            ),
            b_scale_pct: sample_scale_matrix(
                &model.b,
                config.b_scale_min_pct,
                config.b_scale_max_pct,
                &mut rng,
            ),
            noise_seed: rng.next_u64(),
        });
    }
    scenarios
}

fn sample_scale_matrix(
    matrix: &[Vec<f64>],
    min_pct: f64,
    max_pct: f64,
    rng: &mut DeterministicRng,
) -> Vec<Vec<f64>> {
    matrix
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| {
                    if value.abs() <= f64::EPSILON {
                        100.0
                    } else {
                        rng.uniform(min_pct, max_pct)
                    }
                })
                .collect()
        })
        .collect()
}

fn candidate_from_unit(params: &[OptimizableParam], unit_values: &[f64]) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    let mut unit_index = 0usize;
    for param in params {
        let value = if param.fixed {
            param.start
        } else {
            let unit = unit_values.get(unit_index).copied().unwrap_or(0.5);
            unit_index += 1;
            param.min + unit.clamp(0.0, 1.0) * (param.max - param.min)
        };
        out.insert(
            param.key.clone(),
            rounded_controller_value(&param.key, value),
        );
    }
    out
}

fn rounded_controller_value(key: &str, value: f64) -> f64 {
    if key.ends_with("prediction_horizon") || key.ends_with("control_horizon") {
        value.round().max(1.0)
    } else {
        value
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
    let pool = (256usize).max(dim * 64);
    for _ in 0..pool {
        let x = (0..dim).map(|_| rng.next_unit()).collect::<Vec<_>>();
        let k_star = DVector::from_iterator(xs.len(), xs.iter().map(|seen| rbf_kernel(&x, seen)));
        let mu_norm = k_star.dot(&alpha);
        let var = (1.0 - (k_star.transpose() * &k_inv * &k_star)[(0, 0)]).max(1.0e-12);
        let sigma = var.sqrt() * y_scale;
        let mu = mu_norm * y_scale + y_mean;
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
        .zip(right.iter())
        .map(|(l, r)| {
            let d = l - r;
            d * d
        })
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

fn build_inputs(
    logic_io: &IoConfig,
    config: &ProjectConfig,
    simulation: &SimulationConfig,
    current_cv: &HashMap<String, f64>,
    current_mv: &HashMap<String, f64>,
    current_dv: &HashMap<String, f64>,
    params: &HashMap<String, LogicParamValue>,
) -> HashMap<String, SignalValue> {
    let mut values = HashMap::new();
    for point in &logic_io.inputs {
        if let Some(value) = simulated_input_value(
            point.name.as_str(),
            config,
            simulation,
            current_cv,
            current_mv,
            current_dv,
            params,
        ) {
            values.insert(point.name.clone(), value);
        }
    }
    values
}

fn simulated_input_value(
    signal_name: &str,
    config: &ProjectConfig,
    simulation: &SimulationConfig,
    current_cv: &HashMap<String, f64>,
    current_mv: &HashMap<String, f64>,
    current_dv: &HashMap<String, f64>,
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
        ("mv", "read") => Some(SignalValue::F64(*current_mv.get(name).unwrap_or(&0.0))),
        ("mv", "enable") => Some(SignalValue::Bool(true)),
        ("mv", "hl") => simulation_mv_scalar(params, &config.io_definition.mvs, name, "hl")
            .map(SignalValue::F64),
        ("mv", "ll") => simulation_mv_scalar(params, &config.io_definition.mvs, name, "ll")
            .map(SignalValue::F64),
        ("cv", "setpoint") => Some(SignalValue::F64(
            *simulation.cv_setpoints.get(name).unwrap_or(&0.0),
        )),
        ("cv", "process_value") => Some(SignalValue::F64(*current_cv.get(name).unwrap_or(&0.0))),
        ("cv", "enable") => Some(SignalValue::Bool(true)),
        ("cv", "deadband") => {
            simulation_cv_scalar(params, &config.io_definition.cvs, name).map(SignalValue::F64)
        }
        ("dv", "read") => Some(SignalValue::F64(*current_dv.get(name).unwrap_or(&0.0))),
        ("dv", "enable") => Some(SignalValue::Bool(true)),
        _ => None,
    }
}

fn simulation_mv_scalar(
    params: &HashMap<String, LogicParamValue>,
    rows: &[crate::project_config::MvIoDefinition],
    name: &str,
    suffix: &str,
) -> Option<f64> {
    let param_key = format!("io.mv.{name}.{suffix}");
    if let Some(LogicParamValue::F64(value)) = params.get(&param_key) {
        return Some(*value);
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
) -> Option<f64> {
    let param_key = format!("io.cv.{name}.deadband");
    if let Some(LogicParamValue::F64(value)) = params.get(&param_key) {
        return Some(*value);
    }
    rows.iter()
        .find(|row| row.name == name)
        .and_then(|row| row.deadband_value)
}

fn scaled_model_matrices(
    model: &LinearStateSpaceModelConfig,
    simulation: &SimulationConfig,
) -> (Vec<Vec<f64>>, Vec<Vec<f64>>) {
    let mut a = model.a.clone();
    let mut b = model.b.clone();
    for (row_index, row) in a.iter_mut().enumerate() {
        for (col_index, value) in row.iter_mut().enumerate() {
            *value *= simulation
                .a_scale_pct
                .get(row_index)
                .and_then(|row| row.get(col_index))
                .copied()
                .unwrap_or(100.0)
                / 100.0;
        }
    }
    for (row_index, row) in b.iter_mut().enumerate() {
        for (col_index, value) in row.iter_mut().enumerate() {
            *value *= simulation
                .b_scale_pct
                .get(row_index)
                .and_then(|row| row.get(col_index))
                .copied()
                .unwrap_or(100.0)
                / 100.0;
        }
    }
    (a, b)
}

fn initial_u_vector(
    model: &LinearStateSpaceModelConfig,
    simulation: &SimulationConfig,
    vars: &VariableCatalogConfig,
) -> Vec<f64> {
    model
        .u_order
        .iter()
        .map(|name| {
            if vars.mvs.iter().any(|mv| mv == name) {
                *simulation.mv_initials.get(name).unwrap_or(&0.0)
            } else {
                *simulation.dv_initials.get(name).unwrap_or(&0.0)
            }
        })
        .collect()
}

fn current_u_vector(
    model: &LinearStateSpaceModelConfig,
    applied_mv: &HashMap<String, f64>,
    current_dv: &HashMap<String, f64>,
) -> Vec<f64> {
    model
        .u_order
        .iter()
        .map(|name| {
            applied_mv
                .get(name)
                .copied()
                .unwrap_or_else(|| current_dv.get(name).copied().unwrap_or(0.0))
        })
        .collect()
}

fn solve_state_steady_state(a: &[Vec<f64>], b: &[Vec<f64>], u: &[f64]) -> Result<Vec<f64>> {
    let n = a.len();
    let mut matrix = DMatrix::zeros(n, n);
    let mut rhs = DVector::zeros(n);
    for row in 0..n {
        for col in 0..n {
            matrix[(row, col)] = a[row][col];
        }
        rhs[row] = -dot(&b[row], u);
    }
    matrix
        .lu()
        .solve(&rhs)
        .map(|value| value.as_slice().to_vec())
        .context("simulation initial condition solve is singular")
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
    for (name, value) in outputs {
        if let Some(limit) = limits.values.get(name) {
            if let SignalValue::F64(v) = value {
                *v = v.clamp(limit.min, limit.max);
            }
        }
    }
}

fn send_progress(tx: &Option<Sender<OptimizationEvent>>, progress: OptimizationProgress) {
    if let Some(tx) = tx {
        let _ = tx.send(OptimizationEvent::Progress(progress));
    }
}

fn tail_warnings(warnings: &[String]) -> Vec<String> {
    let start = warnings.len().saturating_sub(8);
    warnings[start..].to_vec()
}

#[derive(Debug, Clone)]
struct DeterministicRng {
    state: u64,
    spare: Option<f64>,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.max(1),
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

    fn uniform(&mut self, min: f64, max: f64) -> f64 {
        min + self.next_unit() * (max - min)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_config::{
        CvIoDefinition, IoDefinitionConfig, LinearStateSpaceModelConfig, LogicKind, ModelConfig,
        MvIoDefinition, NumericLimit, ParameterOptimizationParamConfig, ProjectConfig,
        VariableCatalogConfig,
    };

    #[test]
    fn metrics_accumulate_known_signal() {
        let mut metrics = StepMetrics::default();
        accumulate_metric(&mut metrics, 2.0, 0.0, 0.5);
        accumulate_metric(&mut metrics, -2.0, 0.5, 0.5);
        accumulate_overshoot_metric(&mut metrics, 1.5, 0.0, 1.0, 0.5);
        assert!((metrics.ise - 4.0).abs() < 1.0e-9);
        assert!((metrics.iae - 2.0).abs() < 1.0e-9);
        assert!((metrics.itae - 0.5).abs() < 1.0e-9);
        assert!((metrics.overshoot - 0.125).abs() < 1.0e-9);
    }

    #[test]
    fn robust_score_uses_worst_case() {
        assert_eq!(robust_worst_score(&[1.0, 4.0, 2.0]), 4.0);
    }

    #[test]
    fn weighted_score_includes_overshoot_and_actuator() {
        let mut config = ParameterOptimizationConfig::default();
        config.metric_weights.ise = 0.0;
        config.metric_weights.overshoot = 3.0;
        config.metric_weights.actuator = 5.0;
        config.cv_weights.insert("y1".to_string(), 2.0);

        let mut metrics = HashMap::new();
        metrics.insert(
            "y1".to_string(),
            StepMetrics {
                overshoot: 4.0,
                ..StepMetrics::default()
            },
        );

        let score = weighted_score(&config, &metrics, 2.0);
        assert!((score - 34.0).abs() < 1.0e-9);
    }

    #[test]
    fn candidate_builder_keeps_fixed_values() {
        let params = vec![
            OptimizableParam {
                key: "a".to_string(),
                start: 7.0,
                min: 0.0,
                max: 10.0,
                fixed: true,
            },
            OptimizableParam {
                key: "b".to_string(),
                start: 0.0,
                min: 2.0,
                max: 4.0,
                fixed: false,
            },
        ];
        let candidate = candidate_from_unit(&params, &[0.25]);
        assert_eq!(candidate["a"], 7.0);
        assert_eq!(candidate["b"], 2.5);
    }

    #[test]
    fn apply_parameters_updates_p_controller_pairs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = ProjectService::new(temp.path());
        let mut cfg = ProjectConfig::new("demo");
        cfg.variables = VariableCatalogConfig {
            mvs: vec!["u1".to_string()],
            cvs: vec!["y1".to_string()],
            dvs: Vec::new(),
        };
        cfg.io_definition = IoDefinitionConfig {
            mvs: vec![MvIoDefinition {
                name: "u1".to_string(),
                mv_write: "mv.u1.write".to_string(),
                mv_read: "mv.u1.read".to_string(),
                hl_opc: None,
                ll_opc: None,
                hl: Some(10.0),
                ll: Some(-10.0),
                enable: None,
            }],
            cvs: vec![CvIoDefinition {
                name: "y1".to_string(),
                setpoint: "cv.y1.setpoint".to_string(),
                process_value: "cv.y1.process_value".to_string(),
                deadband_value: Some(0.0),
                deadband_opc: None,
                enable: None,
            }],
            dvs: Vec::new(),
        };
        cfg.rebuild_runtime_io_from_definition().expect("io");
        cfg.limits.values.insert(
            "mv.u1.write".to_string(),
            NumericLimit {
                min: -10.0,
                max: 10.0,
                opc_tag: None,
            },
        );
        cfg.logic.kind = LogicKind::PController;
        cfg.logic.p_controller.pairs = vec![crate::project_config::PControllerPairConfig {
            cv: "y1".to_string(),
            mv: "u1".to_string(),
            kp: 1.0,
            bias: 0.0,
        }];
        cfg.sync_active_online_params();
        service.save_config("demo", &cfg).expect("save");

        apply_optimized_parameters(
            &service,
            "demo",
            &HashMap::from([("p.y1.u1.kp".to_string(), 3.0)]),
        )
        .expect("apply");
        let saved = service.load_config("demo").expect("load");
        assert_eq!(saved.logic.p_controller.pairs[0].kp, 3.0);
    }

    #[test]
    fn optimization_params_reject_zero_width_ranges() {
        let cfg = ParameterOptimizationConfig {
            parameters: vec![ParameterOptimizationParamConfig {
                key: "x".to_string(),
                fixed: false,
                start: 1.0,
                min: 1.0,
                max: 1.0,
            }],
            ..ParameterOptimizationConfig::default()
        };
        assert!(optimization_params(&cfg).is_err());
    }

    #[test]
    fn short_linear_mpc_optimization_returns_finite_result() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = ProjectService::new(temp.path());
        let mut cfg = ProjectConfig::new("mpc_demo");
        cfg.variables = VariableCatalogConfig {
            mvs: vec!["u1".to_string()],
            cvs: vec!["y1".to_string()],
            dvs: Vec::new(),
        };
        cfg.io_definition = IoDefinitionConfig {
            mvs: vec![MvIoDefinition {
                name: "u1".to_string(),
                mv_write: "mv.u1.write".to_string(),
                mv_read: "mv.u1.read".to_string(),
                hl_opc: None,
                ll_opc: None,
                hl: Some(10.0),
                ll: Some(-10.0),
                enable: None,
            }],
            cvs: vec![CvIoDefinition {
                name: "y1".to_string(),
                setpoint: "cv.y1.setpoint".to_string(),
                process_value: "cv.y1.process_value".to_string(),
                deadband_value: Some(0.0),
                deadband_opc: None,
                enable: None,
            }],
            dvs: Vec::new(),
        };
        cfg.rebuild_runtime_io_from_definition().expect("io");
        cfg.model = Some(ModelConfig::LinearStateSpace {
            model: LinearStateSpaceModelConfig {
                state_count: 1,
                u_order: vec!["u1".to_string()],
                y_order: vec!["y1".to_string()],
                a: vec![vec![-1.0]],
                b: vec![vec![1.0]],
                c: vec![vec![1.0]],
                d: vec![vec![0.0]],
            },
        });
        cfg.logic.kind = LogicKind::LinearMpc;
        cfg.simulation.cv_initials.insert("y1".to_string(), 0.0);
        cfg.simulation.mv_initials.insert("u1".to_string(), 0.0);
        cfg.simulation.cv_setpoints.insert("y1".to_string(), 1.0);
        cfg.sync_active_online_params();
        cfg.normalize_simulation_config();
        cfg.normalize_optimization_config();
        cfg.optimization.scenario_count = 1;
        cfg.optimization.max_iterations = 2;
        cfg.optimization.initial_samples = 1;
        cfg.optimization.convergence_patience = 2;
        cfg.optimization.plot_interval = 1;
        cfg.optimization.simulation_duration_s = 4.0;
        for row in &mut cfg.optimization.parameters {
            row.fixed = row.key != "mpc.cv.y1.q";
        }
        service.save_config("mpc_demo", &cfg).expect("save");

        let report = run_parameter_optimization(
            &service,
            "mpc_demo",
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .expect("optimization");

        assert!(report.best_score.is_finite());
        assert!(report.best_params.contains_key("mpc.cv.y1.q"));
        assert_eq!(report.curves.cvs.len(), 1);
        assert_eq!(report.curves.cvs[0].scenarios.len(), 1);
        assert_eq!(report.curves.inputs.len(), 1);
        assert_eq!(report.curves.inputs[0].scenarios.len(), 1);
    }
}
