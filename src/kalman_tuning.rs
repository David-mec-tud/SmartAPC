use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, FixedOffset};
use parking_lot::RwLock;
use serde::Serialize;

use crate::{
    linear_mpc::LinearMpcLogic,
    project_config::{
        LinearStateSpaceModelConfig, LogicKind, LogicParamValue, ProjectConfig,
        VariableCatalogConfig,
    },
};

const INTERNAL_KALMAN_PREFIX: &str = "mpc.kalman";
const SHORT_KALMAN_PREFIX: &str = "kalman";
const OBSERVER_PREFIX: &str = "observer";

#[derive(Debug, Clone)]
pub struct KalmanDataset {
    pub file_name: String,
    pub timestamps: Vec<DateTime<FixedOffset>>,
    pub cvs: HashMap<String, Vec<f64>>,
    pub mvs: HashMap<String, Vec<f64>>,
    pub dvs: HashMap<String, Vec<f64>>,
}

impl KalmanDataset {
    pub fn row_count(&self) -> usize {
        self.timestamps.len()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KalmanSeries {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KalmanCvSeries {
    pub name: String,
    pub measurement: Vec<f64>,
    pub estimate: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KalmanTuningResult {
    pub project_name: String,
    pub file_name: String,
    pub timestamps: Vec<String>,
    pub parameters: HashMap<String, f64>,
    pub cvs: Vec<KalmanCvSeries>,
    pub mvs: Vec<KalmanSeries>,
    pub dvs: Vec<KalmanSeries>,
    pub states: Vec<KalmanSeries>,
    pub biases: Vec<KalmanSeries>,
}

#[derive(Debug, Clone, Default)]
pub struct KalmanTuningStore {
    results: Arc<RwLock<HashMap<String, KalmanTuningResult>>>,
}

impl KalmanTuningStore {
    pub fn put(&self, result: KalmanTuningResult) {
        self.results
            .write()
            .insert(result.project_name.clone(), result);
    }

    pub fn get(&self, project_name: &str) -> Option<KalmanTuningResult> {
        self.results.read().get(project_name).cloned()
    }
}

pub fn tuning_store() -> &'static KalmanTuningStore {
    static STORE: OnceLock<KalmanTuningStore> = OnceLock::new();
    STORE.get_or_init(KalmanTuningStore::default)
}

pub fn kalman_parameter_values(cfg: &ProjectConfig) -> Result<HashMap<String, f64>> {
    if !cfg.kalman_tuning_available() {
        bail!(
            "Kalman tuning requires configured linear MPC or custom Rust observer parameters and a valid linear state-space model"
        );
    }
    let rows = config_kalman_parameter_rows(cfg).ok_or_else(|| {
        anyhow::anyhow!("project does not expose a complete Kalman observer parameter set")
    })?;
    rows.into_iter()
        .map(|row| Ok((row.key, row.value)))
        .collect()
}

pub fn is_kalman_parameter(key: &str) -> bool {
    canonical_kalman_key(key).is_some()
}

pub fn validate_kalman_parameters(values: &HashMap<String, f64>) -> Result<()> {
    for (key, value) in values {
        let canonical = canonical_kalman_key(key)
            .ok_or_else(|| anyhow::anyhow!("unknown Kalman observer parameter '{key}'"))?;
        if !value.is_finite() {
            bail!("Kalman parameter '{key}' must be finite");
        }
        if canonical == "mpc.kalman.initial_p" || canonical.ends_with(".r") {
            if *value <= 0.0 {
                bail!("Kalman parameter '{key}' must be > 0");
            }
        } else if *value < 0.0 {
            bail!("Kalman parameter '{key}' must be >= 0");
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct KalmanParameterRow {
    pub key: String,
    pub value: f64,
}

pub fn config_kalman_parameter_rows(cfg: &ProjectConfig) -> Option<Vec<KalmanParameterRow>> {
    let model = cfg
        .model
        .as_ref()
        .and_then(|model| model.linear_state_space())?;
    let prefixes = match cfg.logic.kind {
        LogicKind::LinearMpc => {
            if !cfg.linear_mpc_is_configured_for(&cfg.io) {
                return None;
            }
            [INTERNAL_KALMAN_PREFIX, SHORT_KALMAN_PREFIX, OBSERVER_PREFIX]
        }
        LogicKind::CustomRust => {
            if !cfg.logic.custom_rust.is_compatible_with(&cfg.io) {
                return None;
            }
            [SHORT_KALMAN_PREFIX, OBSERVER_PREFIX, INTERNAL_KALMAN_PREFIX]
        }
        LogicKind::PController => return None,
    };

    let mut rows = Vec::new();
    for canonical in expected_internal_kalman_keys(model, &cfg.variables) {
        let (_, key, value) = prefixes
            .iter()
            .filter_map(|prefix| {
                let candidate = external_key_for_prefix(&canonical, prefix)?;
                let value = match cfg.logic.params.get(&candidate)? {
                    LogicParamValue::F64(value) => *value,
                    LogicParamValue::Bool(_) => return None,
                };
                Some((*prefix, candidate, value))
            })
            .next()?;
        rows.push(KalmanParameterRow { key, value });
    }
    Some(rows)
}

pub fn normalize_kalman_parameters(values: &HashMap<String, f64>) -> Result<HashMap<String, f64>> {
    let mut normalized = HashMap::new();
    for (key, value) in values {
        let canonical = canonical_kalman_key(key)
            .ok_or_else(|| anyhow::anyhow!("unknown Kalman observer parameter '{key}'"))?;
        if let Some(existing) = normalized.insert(canonical.clone(), *value) {
            if (existing - *value).abs() > f64::EPSILON {
                bail!("duplicate aliases for Kalman parameter '{canonical}' disagree");
            }
        }
    }
    validate_kalman_parameters(&normalized)?;
    Ok(normalized)
}

fn expected_internal_kalman_keys(
    model: &LinearStateSpaceModelConfig,
    vars: &VariableCatalogConfig,
) -> Vec<String> {
    let mut keys = vec![format!("{INTERNAL_KALMAN_PREFIX}.initial_p")];
    for index in 0..model.state_count {
        keys.push(format!("{INTERNAL_KALMAN_PREFIX}.state.{}.q", index + 1));
    }
    for cv in &vars.cvs {
        keys.push(format!("{INTERNAL_KALMAN_PREFIX}.cv.{cv}.r"));
        keys.push(format!("{INTERNAL_KALMAN_PREFIX}.cv.{cv}.bias_q"));
    }
    keys
}

fn canonical_kalman_key(key: &str) -> Option<String> {
    if key == format!("{INTERNAL_KALMAN_PREFIX}.initial_p") {
        return Some(key.to_string());
    }
    for prefix in [INTERNAL_KALMAN_PREFIX, SHORT_KALMAN_PREFIX, OBSERVER_PREFIX] {
        if key == format!("{prefix}.initial_p") {
            return Some(format!("{INTERNAL_KALMAN_PREFIX}.initial_p"));
        }
        if let Some(rest) = key.strip_prefix(&format!("{prefix}.")) {
            if rest.starts_with("state.") || rest.starts_with("cv.") {
                return Some(format!("{INTERNAL_KALMAN_PREFIX}.{rest}"));
            }
        }
    }
    None
}

fn external_key_for_prefix(canonical: &str, prefix: &str) -> Option<String> {
    canonical
        .strip_prefix(INTERNAL_KALMAN_PREFIX)
        .map(|suffix| format!("{prefix}{suffix}"))
}

pub fn import_csv(path: &Path, vars: &VariableCatalogConfig) -> Result<KalmanDataset> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
    {
        bail!("Kalman tuning import requires a .csv file");
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("dataset.csv")
        .to_string();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)
        .with_context(|| format!("failed opening CSV '{}'", path.display()))?;
    parse_csv_reader(&mut reader, file_name, vars)
}

/// Parse a browser upload using the same CSV validation as native file import.
pub fn import_csv_bytes(
    file_name: &str,
    bytes: &[u8],
    vars: &VariableCatalogConfig,
) -> Result<KalmanDataset> {
    if !file_name.to_ascii_lowercase().ends_with(".csv") {
        bail!("Kalman tuning import requires a .csv file");
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(bytes);
    parse_csv_reader(&mut reader, file_name.to_string(), vars)
}

fn parse_csv_reader<R: std::io::Read>(
    reader: &mut csv::Reader<R>,
    file_name: String,
    vars: &VariableCatalogConfig,
) -> Result<KalmanDataset> {
    let headers = reader
        .headers()
        .context("failed reading CSV headers")?
        .clone();
    let mut seen = HashSet::new();
    for header in &headers {
        if !seen.insert(header) {
            bail!("duplicate CSV column '{header}'");
        }
    }

    let required = std::iter::once("time")
        .chain(vars.cvs.iter().map(String::as_str))
        .chain(vars.mvs.iter().map(String::as_str))
        .chain(vars.dvs.iter().map(String::as_str));
    for name in required {
        if !headers.iter().any(|header| header == name) {
            bail!("missing required CSV column '{name}'");
        }
    }

    let time_index = header_index(&headers, "time")?;
    let cv_indices = column_indices(&headers, &vars.cvs)?;
    let mv_indices = column_indices(&headers, &vars.mvs)?;
    let dv_indices = column_indices(&headers, &vars.dvs)?;
    let mut timestamps = Vec::new();
    let mut cvs = empty_series(&vars.cvs);
    let mut mvs = empty_series(&vars.mvs);
    let mut dvs = empty_series(&vars.dvs);

    for (row_offset, record) in reader.records().enumerate() {
        let row = row_offset + 2;
        let record = record.with_context(|| format!("failed reading CSV row {row}"))?;
        let raw_time = record
            .get(time_index)
            .ok_or_else(|| anyhow::anyhow!("missing time at CSV row {row}"))?;
        let timestamp = DateTime::parse_from_rfc3339(raw_time)
            .with_context(|| format!("invalid RFC 3339 time at CSV row {row}: '{raw_time}'"))?;
        if timestamps
            .last()
            .is_some_and(|previous| timestamp <= *previous)
        {
            bail!("CSV time must be strictly increasing at row {row}");
        }
        timestamps.push(timestamp);
        push_measurements(&record, row, &cv_indices, &mut cvs)?;
        push_measurements(&record, row, &mv_indices, &mut mvs)?;
        push_measurements(&record, row, &dv_indices, &mut dvs)?;
    }

    if timestamps.len() < 2 {
        bail!("CSV must contain at least two data rows");
    }
    Ok(KalmanDataset {
        file_name,
        timestamps,
        cvs,
        mvs,
        dvs,
    })
}

pub fn run_observer(
    project_name: &str,
    dataset: &KalmanDataset,
    model: &LinearStateSpaceModelConfig,
    vars: &VariableCatalogConfig,
    cycle_ms: u64,
    parameters: &HashMap<String, f64>,
) -> Result<KalmanTuningResult> {
    let display_parameters = parameters.clone();
    let normalized_parameters = normalize_kalman_parameters(parameters)?;
    let logic_params = normalized_parameters
        .iter()
        .map(|(key, value)| (key.clone(), LogicParamValue::F64(*value)))
        .collect();
    let mut observer = LinearMpcLogic::new(
        model.clone(),
        vars.clone(),
        cycle_ms,
        logic_params,
        HashMap::new(),
    );
    let mut cv_estimates = empty_series(&vars.cvs);
    let mut states = (0..model.state_count)
        .map(|index| KalmanSeries {
            name: format!("State {}", index + 1),
            values: Vec::with_capacity(dataset.row_count()),
        })
        .collect::<Vec<_>>();
    let mut biases = vars
        .cvs
        .iter()
        .map(|cv| KalmanSeries {
            name: cv.clone(),
            values: Vec::with_capacity(dataset.row_count()),
        })
        .collect::<Vec<_>>();

    for row in 0..dataset.row_count() {
        let dt_s = if row == 0 {
            cycle_ms as f64 / 1000.0
        } else {
            (dataset.timestamps[row] - dataset.timestamps[row - 1])
                .num_microseconds()
                .context("CSV timestamp interval is outside supported range")? as f64
                / 1_000_000.0
        };
        let measurements = ordered_values(&dataset.cvs, &vars.cvs, row)?;
        let mv_values = ordered_values(&dataset.mvs, &vars.mvs, row)?;
        let dv_values = ordered_values(&dataset.dvs, &vars.dvs, row)?;
        let snapshot =
            observer.replay_observer_step(&measurements, &mv_values, &dv_values, dt_s)?;
        for (index, value) in snapshot.states.iter().enumerate() {
            states[index].values.push(*value);
        }
        for (index, value) in snapshot.biases.iter().enumerate() {
            biases[index].values.push(*value);
        }
        for (index, cv) in vars.cvs.iter().enumerate() {
            cv_estimates
                .get_mut(cv)
                .expect("CV result series")
                .push(snapshot.cv_estimates[index]);
        }
    }

    Ok(KalmanTuningResult {
        project_name: project_name.to_string(),
        file_name: dataset.file_name.clone(),
        timestamps: dataset
            .timestamps
            .iter()
            .map(DateTime::to_rfc3339)
            .collect(),
        parameters: display_parameters,
        cvs: vars
            .cvs
            .iter()
            .map(|name| KalmanCvSeries {
                name: name.clone(),
                measurement: dataset.cvs[name].clone(),
                estimate: cv_estimates[name].clone(),
            })
            .collect(),
        mvs: named_series(&dataset.mvs, &vars.mvs),
        dvs: named_series(&dataset.dvs, &vars.dvs),
        states,
        biases,
    })
}

fn header_index(headers: &csv::StringRecord, name: &str) -> Result<usize> {
    headers
        .iter()
        .position(|header| header == name)
        .ok_or_else(|| anyhow::anyhow!("missing required CSV column '{name}'"))
}

fn column_indices(headers: &csv::StringRecord, names: &[String]) -> Result<Vec<(String, usize)>> {
    names
        .iter()
        .map(|name| Ok((name.clone(), header_index(headers, name)?)))
        .collect()
}

fn empty_series(names: &[String]) -> HashMap<String, Vec<f64>> {
    names
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect()
}

fn push_measurements(
    record: &csv::StringRecord,
    row: usize,
    indices: &[(String, usize)],
    target: &mut HashMap<String, Vec<f64>>,
) -> Result<()> {
    for (name, index) in indices {
        let raw = record
            .get(*index)
            .ok_or_else(|| anyhow::anyhow!("missing '{name}' at CSV row {row}"))?;
        let value = raw
            .parse::<f64>()
            .with_context(|| format!("invalid numeric value for '{name}' at CSV row {row}"))?;
        if !value.is_finite() {
            bail!("non-finite value for '{name}' at CSV row {row}");
        }
        target.get_mut(name).expect("required series").push(value);
    }
    Ok(())
}

fn ordered_values(
    series: &HashMap<String, Vec<f64>>,
    names: &[String],
    row: usize,
) -> Result<Vec<f64>> {
    names
        .iter()
        .map(|name| {
            series
                .get(name)
                .and_then(|values| values.get(row))
                .copied()
                .ok_or_else(|| anyhow::anyhow!("dataset is missing '{name}' at row {}", row + 2))
        })
        .collect()
}

fn named_series(series: &HashMap<String, Vec<f64>>, names: &[String]) -> Vec<KalmanSeries> {
    names
        .iter()
        .map(|name| KalmanSeries {
            name: name.clone(),
            values: series[name].clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear_mpc::linear_mpc_default_params;

    fn vars() -> VariableCatalogConfig {
        VariableCatalogConfig {
            mvs: vec!["u1".to_string()],
            cvs: vec!["y1".to_string()],
            dvs: vec!["d1".to_string()],
        }
    }

    fn parse(text: &str) -> Result<KalmanDataset> {
        let mut reader = csv::Reader::from_reader(text.as_bytes());
        parse_csv_reader(&mut reader, "test.csv".to_string(), &vars())
    }

    #[test]
    fn imports_required_measurements() {
        let data = parse("time,y1,u1,d1\n2026-06-13T12:00:00Z,1,2,3\n2026-06-13T12:00:01Z,2,3,4\n")
            .expect("valid CSV");
        assert_eq!(data.row_count(), 2);
        assert_eq!(data.dvs["d1"], vec![3.0, 4.0]);
    }

    #[test]
    fn rejects_missing_dv_and_non_increasing_time() {
        assert!(parse("time,y1,u1\n2026-06-13T12:00:00Z,1,2\n2026-06-13T12:00:01Z,2,3\n").is_err());
        assert!(
            parse("time,y1,u1,d1\n2026-06-13T12:00:00Z,1,2,3\n2026-06-13T12:00:00Z,2,3,4\n")
                .is_err()
        );
    }

    #[test]
    fn observer_replay_returns_all_requested_series() {
        let vars = VariableCatalogConfig {
            mvs: vec!["u1".to_string()],
            cvs: vec!["y1".to_string()],
            dvs: Vec::new(),
        };
        let model = LinearStateSpaceModelConfig {
            state_count: 1,
            u_order: vec!["u1".to_string()],
            y_order: vec!["y1".to_string()],
            a: vec![vec![-1.0]],
            b: vec![vec![1.0]],
            c: vec![vec![1.0]],
            d: vec![vec![0.0]],
        };
        let dataset = KalmanDataset {
            file_name: "replay.csv".to_string(),
            timestamps: vec![
                DateTime::parse_from_rfc3339("2026-06-13T12:00:00Z").expect("time"),
                DateTime::parse_from_rfc3339("2026-06-13T12:00:00.500Z").expect("time"),
                DateTime::parse_from_rfc3339("2026-06-13T12:00:02Z").expect("time"),
            ],
            cvs: HashMap::from([("y1".to_string(), vec![0.0, 0.4, 0.8])]),
            mvs: HashMap::from([("u1".to_string(), vec![0.0, 1.0, 1.0])]),
            dvs: HashMap::new(),
        };
        let parameters = linear_mpc_default_params(&model, &vars, &HashMap::new())
            .into_iter()
            .filter(|(key, _)| is_kalman_parameter(key))
            .map(|(key, value)| match value {
                LogicParamValue::F64(value) => (key, value),
                LogicParamValue::Bool(_) => unreachable!(),
            })
            .collect();

        let result =
            run_observer("demo", &dataset, &model, &vars, 1_000, &parameters).expect("replay");
        assert_eq!(result.cvs[0].measurement.len(), 3);
        assert_eq!(result.cvs[0].estimate.len(), 3);
        assert_eq!(result.mvs[0].values.len(), 3);
        assert_eq!(result.states[0].values.len(), 3);
        assert_eq!(result.biases[0].values.len(), 3);
    }

    #[test]
    fn observer_aliases_normalize_to_internal_kalman_keys() {
        let values = HashMap::from([
            ("observer.initial_p".to_string(), 1.0),
            ("observer.state.1.q".to_string(), 1.0e-4),
            ("observer.cv.y1.r".to_string(), 1.0e-2),
            ("observer.cv.y1.bias_q".to_string(), 1.0e-3),
        ]);
        let normalized = normalize_kalman_parameters(&values).expect("normalized aliases");
        assert_eq!(normalized.get("mpc.kalman.initial_p"), Some(&1.0));
        assert_eq!(normalized.get("mpc.kalman.cv.y1.bias_q"), Some(&1.0e-3));
    }
}
