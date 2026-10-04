use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::Instant,
};

use anyhow::{Context, Result, bail};

use crate::{
    project_config::{LogicParamType, LogicParamValue, ProjectConfig},
    runtime::build_logic,
    types::{RuntimeState, SignalType, SignalValue},
};

const DEFAULT_NUMERIC_SAMPLES: usize = 3;
const MAX_FULL_FACTORIAL_CASES: usize = 4_096;
const MAX_PREVIEW_CASES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SweepDimensionKind {
    Input,
    Parameter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SweepValueKind {
    F64,
    Bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DefaultSweepDimension {
    pub kind: SweepDimensionKind,
    pub name: String,
    pub value_kind: SweepValueKind,
    pub baseline_label: String,
    pub default_min: Option<f64>,
    pub default_max: Option<f64>,
    pub default_samples: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NumericSweepConfig {
    pub min: f64,
    pub max: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SweepSelection {
    pub kind: SweepDimensionKind,
    pub name: String,
    pub enabled: bool,
    pub numeric: Option<NumericSweepConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SweepExecutionMode {
    FullFactorial,
    SingleAxisFallback,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SweepExecutionSummary {
    pub mode: SweepExecutionMode,
    pub full_factorial_case_count: usize,
    pub executed_case_count: usize,
    pub duration_min_us: u64,
    pub duration_avg_us: f64,
    pub duration_p95_us: u64,
    pub duration_max_us: u64,
    pub total_wall_time_ms: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseCountSummary {
    pub ok_cases: usize,
    pub warning_cases: usize,
    pub error_cases: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputValidationSummary {
    pub invalid_output_cases: usize,
    pub out_of_limit_cases: usize,
    pub unexpected_output_cases: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CasePreview {
    pub label: String,
    pub duration_us: u64,
    pub status: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ControlFunctionTestReport {
    pub execution: SweepExecutionSummary,
    pub counts: CaseCountSummary,
    pub outputs: OutputValidationSummary,
    pub slowest_cases: Vec<CasePreview>,
    pub issue_cases: Vec<CasePreview>,
}

#[derive(Debug, Clone)]
struct PreparedDimension {
    kind: SweepDimensionKind,
    name: String,
    values: Vec<SweepScalarValue>,
}

#[derive(Debug, Clone, PartialEq)]
enum SweepScalarValue {
    F64(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
struct CaseEvaluation {
    label: String,
    duration_us: u64,
    status: CaseStatus,
    details: Vec<String>,
    has_invalid_output: bool,
    has_out_of_limit_output: bool,
    has_unexpected_output: bool,
}

pub fn default_sweep_dimensions(config: &ProjectConfig) -> Vec<DefaultSweepDimension> {
    let mut cfg = config.clone();
    cfg.sync_active_online_params();

    let baseline_inputs = default_input_values(&cfg);
    let active_params = cfg.logic.params.clone();

    let mut dimensions = Vec::new();

    for input in &cfg.io.inputs {
        match input.signal_type {
            SignalType::F64 => {
                let baseline = baseline_inputs
                    .get(&input.name)
                    .and_then(SignalValue::as_f64)
                    .unwrap_or(0.0);
                let (min, max) = default_numeric_input_range(&cfg, &input.name, baseline);
                dimensions.push(DefaultSweepDimension {
                    kind: SweepDimensionKind::Input,
                    name: input.name.clone(),
                    value_kind: SweepValueKind::F64,
                    baseline_label: format_numeric(baseline),
                    default_min: Some(min),
                    default_max: Some(max),
                    default_samples: DEFAULT_NUMERIC_SAMPLES,
                });
            }
            SignalType::Bool => {
                let baseline = baseline_inputs
                    .get(&input.name)
                    .and_then(SignalValue::as_bool)
                    .unwrap_or(false);
                dimensions.push(DefaultSweepDimension {
                    kind: SweepDimensionKind::Input,
                    name: input.name.clone(),
                    value_kind: SweepValueKind::Bool,
                    baseline_label: baseline.to_string(),
                    default_min: None,
                    default_max: None,
                    default_samples: 2,
                });
            }
        }
    }

    let mut param_names: Vec<_> = active_params.keys().cloned().collect();
    param_names.sort();
    for name in param_names {
        let Some(value) = active_params.get(&name) else {
            continue;
        };
        match value {
            LogicParamValue::F64(v) => {
                let (min, max) = default_numeric_param_range(&name, *v);
                dimensions.push(DefaultSweepDimension {
                    kind: SweepDimensionKind::Parameter,
                    name,
                    value_kind: SweepValueKind::F64,
                    baseline_label: format_numeric(*v),
                    default_min: Some(min),
                    default_max: Some(max),
                    default_samples: DEFAULT_NUMERIC_SAMPLES,
                });
            }
            LogicParamValue::Bool(v) => {
                dimensions.push(DefaultSweepDimension {
                    kind: SweepDimensionKind::Parameter,
                    name,
                    value_kind: SweepValueKind::Bool,
                    baseline_label: v.to_string(),
                    default_min: None,
                    default_max: None,
                    default_samples: 2,
                });
            }
        }
    }

    dimensions
}

pub fn run_control_function_tests(
    config: &ProjectConfig,
    project_dir: &Path,
    selections: &[SweepSelection],
) -> Result<ControlFunctionTestReport> {
    let mut cfg = config.clone();
    cfg.sync_active_online_params();

    let baseline_inputs = default_input_values(&cfg);
    let baseline_params = cfg.logic.params.clone();
    let prepared = prepare_dimensions(&cfg, selections, &baseline_inputs, &baseline_params)?;

    let full_factorial_case_count = if prepared.is_empty() {
        1
    } else {
        prepared
            .iter()
            .fold(1usize, |count, dim| count.saturating_mul(dim.values.len()))
    };
    let mode = if full_factorial_case_count <= MAX_FULL_FACTORIAL_CASES {
        SweepExecutionMode::FullFactorial
    } else {
        SweepExecutionMode::SingleAxisFallback
    };

    let mut logic = build_logic(&cfg, &cfg.io, project_dir)
        .with_context(|| "active control logic could not be loaded for offline tests")?;

    let suite_started = Instant::now();
    let mut evaluations = Vec::new();

    evaluations.push(run_case(
        &cfg,
        logic.as_mut(),
        "baseline".to_string(),
        &baseline_inputs,
        &baseline_params,
    ));

    match mode {
        SweepExecutionMode::FullFactorial => {
            let mut current_inputs = baseline_inputs.clone();
            let mut current_params = baseline_params.clone();
            let mut current_assignments = Vec::new();
            execute_full_factorial_cases(
                &cfg,
                logic.as_mut(),
                &prepared,
                0,
                &mut current_inputs,
                &mut current_params,
                &mut current_assignments,
                &mut evaluations,
            );
        }
        SweepExecutionMode::SingleAxisFallback => {
            for dimension in &prepared {
                for value in &dimension.values {
                    let mut inputs = baseline_inputs.clone();
                    let mut params = baseline_params.clone();
                    assign_dimension_value(
                        &dimension.kind,
                        &dimension.name,
                        value,
                        &mut inputs,
                        &mut params,
                    );
                    evaluations.push(run_case(
                        &cfg,
                        logic.as_mut(),
                        format!("{} = {}", dimension.name, value.label()),
                        &inputs,
                        &params,
                    ));
                }
            }
        }
    }

    let total_wall_time_ms = suite_started.elapsed().as_secs_f64() * 1_000.0;
    Ok(summarize_report(
        mode,
        full_factorial_case_count,
        total_wall_time_ms,
        evaluations,
    ))
}

fn prepare_dimensions(
    config: &ProjectConfig,
    selections: &[SweepSelection],
    baseline_inputs: &HashMap<String, SignalValue>,
    baseline_params: &HashMap<String, LogicParamValue>,
) -> Result<Vec<PreparedDimension>> {
    let input_types: HashMap<_, _> = config
        .io
        .inputs
        .iter()
        .map(|point| (point.name.as_str(), point.signal_type))
        .collect();
    let param_types: HashMap<_, _> = baseline_params
        .iter()
        .map(|(key, value)| {
            (
                key.as_str(),
                match value {
                    LogicParamValue::F64(_) => LogicParamType::F64,
                    LogicParamValue::Bool(_) => LogicParamType::Bool,
                },
            )
        })
        .collect();

    let mut seen = HashSet::new();
    let mut prepared = Vec::new();

    for selection in selections {
        if !selection.enabled {
            continue;
        }

        let key = (selection.kind, selection.name.as_str());
        if !seen.insert(key) {
            bail!("duplicate sweep selection '{}'", selection.name);
        }

        match selection.kind {
            SweepDimensionKind::Input => {
                let Some(signal_type) = input_types.get(selection.name.as_str()) else {
                    bail!("unknown input sweep '{}'", selection.name);
                };
                let values = match signal_type {
                    SignalType::Bool => {
                        vec![SweepScalarValue::Bool(false), SweepScalarValue::Bool(true)]
                    }
                    SignalType::F64 => {
                        numeric_sweep_values(selection.numeric.as_ref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "numeric input sweep '{}' requires min/max/sample configuration",
                                selection.name
                            )
                        })?)?
                        .into_iter()
                        .map(SweepScalarValue::F64)
                        .collect()
                    }
                };
                if baseline_inputs.get(&selection.name).is_none() {
                    bail!("input baseline missing for '{}'", selection.name);
                }
                prepared.push(PreparedDimension {
                    kind: SweepDimensionKind::Input,
                    name: selection.name.clone(),
                    values,
                });
            }
            SweepDimensionKind::Parameter => {
                let Some(param_type) = param_types.get(selection.name.as_str()) else {
                    bail!("unknown parameter sweep '{}'", selection.name);
                };
                let values = match param_type {
                    LogicParamType::Bool => {
                        vec![SweepScalarValue::Bool(false), SweepScalarValue::Bool(true)]
                    }
                    LogicParamType::F64 => numeric_sweep_values(
                        selection.numeric.as_ref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "numeric parameter sweep '{}' requires min/max/sample configuration",
                                selection.name
                            )
                        })?,
                    )?
                    .into_iter()
                    .map(SweepScalarValue::F64)
                    .collect(),
                };
                prepared.push(PreparedDimension {
                    kind: SweepDimensionKind::Parameter,
                    name: selection.name.clone(),
                    values,
                });
            }
        }
    }

    Ok(prepared)
}

fn numeric_sweep_values(config: &NumericSweepConfig) -> Result<Vec<f64>> {
    if !config.min.is_finite() || !config.max.is_finite() {
        bail!("numeric sweep range must be finite");
    }
    if config.samples == 0 {
        bail!("numeric sweep sample count must be > 0");
    }

    if config.samples == 1 || (config.max - config.min).abs() < f64::EPSILON {
        return Ok(vec![config.min]);
    }

    let mut values = Vec::with_capacity(config.samples);
    let span = config.max - config.min;
    let step = span / (config.samples - 1) as f64;
    for index in 0..config.samples {
        values.push(config.min + step * index as f64);
    }

    values.dedup_by(|left, right| (*left - *right).abs() < f64::EPSILON);
    Ok(values)
}

fn execute_full_factorial_cases(
    config: &ProjectConfig,
    logic: &mut dyn crate::logic::ControlLogic,
    dimensions: &[PreparedDimension],
    index: usize,
    current_inputs: &mut HashMap<String, SignalValue>,
    current_params: &mut HashMap<String, LogicParamValue>,
    current_assignments: &mut Vec<String>,
    evaluations: &mut Vec<CaseEvaluation>,
) {
    if index >= dimensions.len() {
        evaluations.push(run_case(
            config,
            logic,
            current_assignments.join(", "),
            current_inputs,
            current_params,
        ));
        return;
    }

    let dimension = &dimensions[index];
    for value in &dimension.values {
        assign_dimension_value(
            &dimension.kind,
            &dimension.name,
            value,
            current_inputs,
            current_params,
        );
        current_assignments.push(format!("{} = {}", dimension.name, value.label()));
        execute_full_factorial_cases(
            config,
            logic,
            dimensions,
            index + 1,
            current_inputs,
            current_params,
            current_assignments,
            evaluations,
        );
        let _ = current_assignments.pop();
    }
}

fn run_case(
    config: &ProjectConfig,
    logic: &mut dyn crate::logic::ControlLogic,
    label: String,
    inputs: &HashMap<String, SignalValue>,
    params: &HashMap<String, LogicParamValue>,
) -> CaseEvaluation {
    logic.init(config.logic.initial_values.clone(), params.clone());

    let started = Instant::now();
    let result = logic.step(inputs, RuntimeState::Running);
    let duration_us = started.elapsed().as_micros() as u64;

    match result {
        Ok(outputs) => validate_outputs(config, label, duration_us, &outputs),
        Err(err) => CaseEvaluation {
            label,
            duration_us,
            status: CaseStatus::Error,
            details: vec![err.to_string()],
            has_invalid_output: false,
            has_out_of_limit_output: false,
            has_unexpected_output: false,
        },
    }
}

fn validate_outputs(
    config: &ProjectConfig,
    label: String,
    duration_us: u64,
    outputs: &HashMap<String, SignalValue>,
) -> CaseEvaluation {
    let output_types: HashMap<_, _> = config
        .io
        .outputs
        .iter()
        .map(|point| (point.name.as_str(), point.signal_type))
        .collect();

    let mut details = Vec::new();
    let mut has_invalid_output = false;
    let mut has_out_of_limit_output = false;
    let mut has_unexpected_output = false;

    for (name, value) in outputs {
        let Some(expected_type) = output_types.get(name.as_str()) else {
            has_unexpected_output = true;
            details.push(format!(
                "unexpected output '{}' returned by control function",
                name
            ));
            continue;
        };

        match (expected_type, value) {
            (SignalType::F64, SignalValue::F64(v)) => {
                if !v.is_finite() {
                    has_invalid_output = true;
                    details.push(format!("output '{}' produced non-finite value {}", name, v));
                    continue;
                }
                if let Some(limit) = config.limits.values.get(name) {
                    if *v < limit.min || *v > limit.max {
                        has_out_of_limit_output = true;
                        details.push(format!(
                            "output '{}' left configured limits [{}, {}] with {}",
                            name,
                            format_numeric(limit.min),
                            format_numeric(limit.max),
                            format_numeric(*v)
                        ));
                    }
                }
            }
            (SignalType::Bool, SignalValue::Bool(_)) => {}
            _ => {
                has_invalid_output = true;
                details.push(format!(
                    "output '{}' has wrong type for configured runtime I/O",
                    name
                ));
            }
        }
    }

    let status = if has_invalid_output {
        CaseStatus::Error
    } else if has_out_of_limit_output || has_unexpected_output {
        CaseStatus::Warning
    } else {
        CaseStatus::Ok
    };

    CaseEvaluation {
        label,
        duration_us,
        status,
        details,
        has_invalid_output,
        has_out_of_limit_output,
        has_unexpected_output,
    }
}

fn summarize_report(
    mode: SweepExecutionMode,
    full_factorial_case_count: usize,
    total_wall_time_ms: f64,
    evaluations: Vec<CaseEvaluation>,
) -> ControlFunctionTestReport {
    let mut ok_cases = 0usize;
    let mut warning_cases = 0usize;
    let mut error_cases = 0usize;
    let mut invalid_output_cases = 0usize;
    let mut out_of_limit_cases = 0usize;
    let mut unexpected_output_cases = 0usize;
    let mut durations = Vec::with_capacity(evaluations.len());
    let mut slowest = evaluations.clone();
    let mut issue_cases = Vec::new();

    for evaluation in &evaluations {
        durations.push(evaluation.duration_us);
        match evaluation.status {
            CaseStatus::Ok => ok_cases += 1,
            CaseStatus::Warning => warning_cases += 1,
            CaseStatus::Error => error_cases += 1,
        }
        if evaluation.has_invalid_output {
            invalid_output_cases += 1;
        }
        if evaluation.has_out_of_limit_output {
            out_of_limit_cases += 1;
        }
        if evaluation.has_unexpected_output {
            unexpected_output_cases += 1;
        }
        if !evaluation.details.is_empty() && issue_cases.len() < MAX_PREVIEW_CASES {
            issue_cases.push(to_preview(evaluation));
        }
    }

    durations.sort_unstable();
    slowest.sort_by_key(|evaluation| std::cmp::Reverse(evaluation.duration_us));

    ControlFunctionTestReport {
        execution: SweepExecutionSummary {
            mode,
            full_factorial_case_count,
            executed_case_count: evaluations.len(),
            duration_min_us: durations.first().copied().unwrap_or(0),
            duration_avg_us: average_u64(&durations),
            duration_p95_us: percentile_u64(&durations, 95),
            duration_max_us: durations.last().copied().unwrap_or(0),
            total_wall_time_ms,
        },
        counts: CaseCountSummary {
            ok_cases,
            warning_cases,
            error_cases,
        },
        outputs: OutputValidationSummary {
            invalid_output_cases,
            out_of_limit_cases,
            unexpected_output_cases,
        },
        slowest_cases: slowest
            .iter()
            .take(MAX_PREVIEW_CASES)
            .map(to_preview)
            .collect(),
        issue_cases,
    }
}

fn to_preview(evaluation: &CaseEvaluation) -> CasePreview {
    CasePreview {
        label: evaluation.label.clone(),
        duration_us: evaluation.duration_us,
        status: match evaluation.status {
            CaseStatus::Ok => "ok".to_string(),
            CaseStatus::Warning => "warning".to_string(),
            CaseStatus::Error => "error".to_string(),
        },
        details: evaluation.details.clone(),
    }
}

fn percentile_u64(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let index = ((values.len() - 1) * percentile) / 100;
    values[index]
}

fn average_u64(values: &[u64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: u128 = values.iter().map(|value| *value as u128).sum();
    sum as f64 / values.len() as f64
}

fn assign_dimension_value(
    kind: &SweepDimensionKind,
    name: &str,
    value: &SweepScalarValue,
    inputs: &mut HashMap<String, SignalValue>,
    params: &mut HashMap<String, LogicParamValue>,
) {
    match kind {
        SweepDimensionKind::Input => {
            inputs.insert(name.to_string(), value.as_signal_value());
        }
        SweepDimensionKind::Parameter => {
            params.insert(name.to_string(), value.as_logic_param_value());
        }
    }
}

fn default_input_values(config: &ProjectConfig) -> HashMap<String, SignalValue> {
    let mut values = HashMap::new();

    for input in &config.io.inputs {
        let value = match input.signal_type {
            SignalType::Bool => SignalValue::Bool(input.name.ends_with(".enable")),
            SignalType::F64 => {
                SignalValue::F64(default_numeric_input_baseline(config, &input.name))
            }
        };
        values.insert(input.name.clone(), value);
    }

    values
}

fn default_numeric_input_baseline(config: &ProjectConfig, signal_name: &str) -> f64 {
    if let Some(rest) = signal_name.strip_prefix("mv.") {
        let mut parts = rest.split('.');
        if let (Some(name), Some(suffix)) = (parts.next(), parts.next()) {
            if let Some(row) = config.io_definition.mvs.iter().find(|row| row.name == name) {
                let midpoint = midpoint_from_bounds(row.ll, row.hl).unwrap_or(0.0);
                return match suffix {
                    "read" | "hl" | "ll" => midpoint,
                    _ => midpoint,
                };
            }
        }
    }

    if let Some(rest) = signal_name.strip_prefix("cv.") {
        let mut parts = rest.split('.');
        if let (Some(name), Some(suffix)) = (parts.next(), parts.next()) {
            if let Some(row) = config.io_definition.cvs.iter().find(|row| row.name == name) {
                return match suffix {
                    "deadband" => row.deadband_value.unwrap_or(0.0),
                    _ => 0.0,
                };
            }
        }
    }

    0.0
}

fn default_numeric_input_range(
    config: &ProjectConfig,
    signal_name: &str,
    baseline: f64,
) -> (f64, f64) {
    if let Some(rest) = signal_name.strip_prefix("mv.") {
        let mut parts = rest.split('.');
        if let (Some(name), Some(_suffix)) = (parts.next(), parts.next()) {
            if let Some(row) = config.io_definition.mvs.iter().find(|row| row.name == name) {
                if let Some((min, max)) = normalize_bounds(row.ll, row.hl) {
                    return (min, max);
                }
            }
        }
    }

    if let Some(rest) = signal_name.strip_prefix("cv.") {
        let mut parts = rest.split('.');
        if let (Some(name), Some(suffix)) = (parts.next(), parts.next()) {
            if let Some(row) = config.io_definition.cvs.iter().find(|row| row.name == name) {
                if suffix == "deadband" {
                    let band = row.deadband_value.unwrap_or(1.0).abs().max(1.0);
                    return (0.0, band * 2.0);
                }
                if let Some(band) = row.deadband_value {
                    return symmetric_range(baseline, band.abs().max(1.0), false);
                }
            }
        }
    }

    default_numeric_range(signal_name, baseline)
}

fn default_numeric_param_range(name: &str, baseline: f64) -> (f64, f64) {
    default_numeric_range(name, baseline)
}

fn default_numeric_range(name: &str, baseline: f64) -> (f64, f64) {
    let non_negative = name.ends_with(".kp") || name.ends_with(".deadband");
    let span = baseline.abs().max(1.0);
    symmetric_range(baseline, span, non_negative)
}

fn symmetric_range(center: f64, span: f64, non_negative: bool) -> (f64, f64) {
    let mut min = center - span;
    let max = center + span;
    if non_negative {
        min = min.max(0.0);
    }
    if min <= max { (min, max) } else { (max, min) }
}

fn midpoint_from_bounds(min: Option<f64>, max: Option<f64>) -> Option<f64> {
    normalize_bounds(min, max).map(|(min, max)| (min + max) / 2.0)
}

fn normalize_bounds(min: Option<f64>, max: Option<f64>) -> Option<(f64, f64)> {
    let (mut min, mut max) = (min?, max?);
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    Some((min, max))
}

fn format_numeric(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
    }
}

impl SweepScalarValue {
    fn as_signal_value(&self) -> SignalValue {
        match self {
            Self::F64(v) => SignalValue::F64(*v),
            Self::Bool(v) => SignalValue::Bool(*v),
        }
    }

    fn as_logic_param_value(&self) -> LogicParamValue {
        match self {
            Self::F64(v) => LogicParamValue::F64(*v),
            Self::Bool(v) => LogicParamValue::Bool(*v),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::F64(v) => format_numeric(*v),
            Self::Bool(v) => v.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::project_config::{
        CvIoDefinition, IoDefinitionConfig, LogicConfig, LogicKind, MvIoDefinition,
        PControllerLogicConfig, PControllerPairConfig, ProjectConfig, VariableCatalogConfig,
    };

    fn base_config() -> ProjectConfig {
        let mut cfg = ProjectConfig::new("control_function_tests");
        cfg.variables = VariableCatalogConfig {
            mvs: vec!["u1".to_string()],
            cvs: vec!["y1".to_string()],
            dvs: Vec::new(),
        };
        cfg.io_definition = IoDefinitionConfig {
            mvs: vec![MvIoDefinition {
                name: "u1".to_string(),
                mv_write: "ns=2;s=u1_write".to_string(),
                mv_read: "ns=2;s=u1_read".to_string(),
                hl_opc: None,
                ll_opc: None,
                hl: Some(10.0),
                ll: Some(-10.0),
                enable: None,
            }],
            cvs: vec![CvIoDefinition {
                name: "y1".to_string(),
                setpoint: "ns=2;s=y1_setpoint".to_string(),
                process_value: "ns=2;s=y1_ist".to_string(),
                hl: Some(100.0),
                ll: Some(-100.0),
                deadband_opc: None,
                deadband_value: Some(2.0),
                enable: Some("ns=2;s=y1_enable".to_string()),
            }],
            dvs: Vec::new(),
        };
        cfg.rebuild_runtime_io_from_definition()
            .expect("runtime io");
        cfg.logic = LogicConfig {
            cycle_ms: 1_000,
            kind: LogicKind::PController,
            p_controller: PControllerLogicConfig {
                pairs: vec![PControllerPairConfig {
                    cv: "y1".to_string(),
                    mv: "u1".to_string(),
                    kp: 2.0,
                    bias: 0.0,
                }],
            },
            custom_rust: Default::default(),
            params: HashMap::new(),
            initial_values: HashMap::new(),
        };
        cfg.sync_active_online_params();
        cfg
    }

    #[test]
    fn default_dimensions_include_inputs_and_params() {
        let cfg = base_config();
        let dimensions = default_sweep_dimensions(&cfg);

        assert!(dimensions.iter().any(|dim| dim.name == "cv.y1.setpoint"));
        assert!(dimensions.iter().any(|dim| dim.name == "cv.y1.enable"));
        assert!(dimensions.iter().any(|dim| dim.name == "p.y1.u1.kp"));

        let mv_read = dimensions
            .iter()
            .find(|dim| dim.name == "mv.u1.read")
            .expect("mv read dimension");
        assert_eq!(mv_read.default_min, Some(-10.0));
        assert_eq!(mv_read.default_max, Some(10.0));
    }

    #[test]
    fn reports_output_limit_warnings_for_p_controller() {
        let mut cfg = base_config();
        cfg.logic
            .params
            .insert("p.y1.u1.kp".to_string(), LogicParamValue::F64(20.0));

        let selections = vec![
            SweepSelection {
                kind: SweepDimensionKind::Input,
                name: "cv.y1.setpoint".to_string(),
                enabled: true,
                numeric: Some(NumericSweepConfig {
                    min: -2.0,
                    max: 2.0,
                    samples: 3,
                }),
            },
            SweepSelection {
                kind: SweepDimensionKind::Input,
                name: "cv.y1.process_value".to_string(),
                enabled: true,
                numeric: Some(NumericSweepConfig {
                    min: -2.0,
                    max: 2.0,
                    samples: 3,
                }),
            },
        ];

        let report = run_control_function_tests(&cfg, std::path::Path::new("."), &selections)
            .expect("report");

        assert!(report.execution.executed_case_count >= 2);
        assert!(report.outputs.out_of_limit_cases > 0);
    }

    #[test]
    fn falls_back_when_full_factorial_space_is_too_large() {
        let cfg = base_config();
        let selections = vec![
            SweepSelection {
                kind: SweepDimensionKind::Input,
                name: "cv.y1.setpoint".to_string(),
                enabled: true,
                numeric: Some(NumericSweepConfig {
                    min: -1.0,
                    max: 1.0,
                    samples: 20,
                }),
            },
            SweepSelection {
                kind: SweepDimensionKind::Input,
                name: "cv.y1.process_value".to_string(),
                enabled: true,
                numeric: Some(NumericSweepConfig {
                    min: -1.0,
                    max: 1.0,
                    samples: 20,
                }),
            },
            SweepSelection {
                kind: SweepDimensionKind::Parameter,
                name: "p.y1.u1.kp".to_string(),
                enabled: true,
                numeric: Some(NumericSweepConfig {
                    min: 0.0,
                    max: 10.0,
                    samples: 20,
                }),
            },
        ];

        let report = run_control_function_tests(&cfg, std::path::Path::new("."), &selections)
            .expect("report");

        assert_eq!(
            report.execution.mode,
            SweepExecutionMode::SingleAxisFallback
        );
        assert!(report.execution.full_factorial_case_count > MAX_FULL_FACTORIAL_CASES);
    }
}
