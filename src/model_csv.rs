//! One CSV contains labeled A/B/C/D blocks for the current browser draft.
//! Import is atomic: no model is returned until every label and value is valid.
use anyhow::{Result, bail, ensure};
use serde::Deserialize;

use crate::project_config::{LinearStateSpaceModelConfig, VariableCatalogConfig};

#[derive(Debug, Deserialize)]
pub(crate) struct ModelCsvLayout {
    pub state_count: usize,
    pub variables: VariableCatalogConfig,
}

struct MatrixLayout {
    name: &'static str,
    rows: Vec<String>,
    columns: Vec<String>,
}

impl ModelCsvLayout {
    fn matrices(&self) -> Result<[MatrixLayout; 4]> {
        ensure!(
            (1..=100).contains(&self.state_count),
            "State dimensions must be between 1 and 100"
        );
        self.variables.validate()?;
        let states: Vec<_> = (1..=self.state_count).map(|n| format!("x{n}")).collect();
        let inputs: Vec<_> = self
            .variables
            .mvs
            .iter()
            .chain(&self.variables.dvs)
            .cloned()
            .collect();
        Ok([
            MatrixLayout {
                name: "A",
                rows: states.clone(),
                columns: states.clone(),
            },
            MatrixLayout {
                name: "B",
                rows: states.clone(),
                columns: inputs.clone(),
            },
            MatrixLayout {
                name: "C",
                rows: self.variables.cvs.clone(),
                columns: states,
            },
            MatrixLayout {
                name: "D",
                rows: self.variables.cvs.clone(),
                columns: inputs,
            },
        ])
    }

    pub fn template(&self) -> Result<String> {
        let mut writer = csv::WriterBuilder::new()
            .flexible(true)
            .from_writer(Vec::new());
        for matrix in self.matrices()? {
            writer.write_record(
                std::iter::once(matrix.name).chain(matrix.columns.iter().map(String::as_str)),
            )?;
            for (row, label) in matrix.rows.iter().enumerate() {
                let coefficients = (1..=matrix.columns.len()).map(|column| {
                    format!(
                        "{}{row_number}{column}",
                        matrix.name.to_lowercase(),
                        row_number = row + 1
                    )
                });
                writer.write_record(std::iter::once(label.clone()).chain(coefficients))?;
            }
            writer.write_record([""])?;
        }
        Ok(String::from_utf8(writer.into_inner()?)?)
    }

    pub fn import(&self, content: &str) -> Result<LinearStateSpaceModelConfig> {
        let layouts = self.matrices()?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(content.as_bytes());
        let records = reader
            .records()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut records = records
            .iter()
            .filter(|record| !record.iter().all(|cell| cell.trim().is_empty()));
        let mut values = Vec::new();
        for matrix in layouts {
            let header = records
                .next()
                .ok_or_else(|| anyhow::anyhow!("Missing {} matrix header", matrix.name))?;
            let expected: Vec<_> = std::iter::once(matrix.name)
                .chain(matrix.columns.iter().map(String::as_str))
                .collect();
            ensure!(
                matches_record(header, &expected),
                "{} header must match the current state/signal order: {}",
                matrix.name,
                expected.join(", ")
            );
            let mut rows = Vec::new();
            for label in &matrix.rows {
                let record = records
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Missing {} row {label}", matrix.name))?;
                ensure!(
                    record.get(0) == Some(label.as_str()),
                    "{} row label must be {label}",
                    matrix.name
                );
                ensure!(
                    record.len() >= expected.len()
                        && record
                            .iter()
                            .skip(expected.len())
                            .all(|cell| cell.trim().is_empty()),
                    "{} row {label} requires {} numeric values",
                    matrix.name,
                    matrix.columns.len()
                );
                let mut row = Vec::new();
                for (column, cell) in matrix.columns.iter().zip(record.iter().skip(1)) {
                    let value = cell
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite());
                    let Some(value) = value else {
                        bail!(
                            "{} row {label}, column {column} must contain a finite number; replace all placeholders",
                            matrix.name
                        );
                    };
                    row.push(value);
                }
                rows.push(row);
            }
            values.push(rows);
        }
        ensure!(records.next().is_none(), "Unexpected rows after D matrix");
        let mut matrices = values.into_iter();
        let model = LinearStateSpaceModelConfig {
            state_count: self.state_count,
            u_order: self
                .variables
                .mvs
                .iter()
                .chain(&self.variables.dvs)
                .cloned()
                .collect(),
            y_order: self.variables.cvs.clone(),
            a: matrices.next().unwrap(),
            b: matrices.next().unwrap(),
            c: matrices.next().unwrap(),
            d: matrices.next().unwrap(),
        };
        model.validate(&self.variables)?;
        Ok(model)
    }
}

/// Spreadsheet editors pad shorter blocks with empty columns when saving.
/// Ignore only surplus empty cells; missing/blank coefficients still fail.
fn matches_record(record: &csv::StringRecord, expected: &[&str]) -> bool {
    record.len() >= expected.len()
        && record
            .iter()
            .take(expected.len())
            .eq(expected.iter().copied())
        && record
            .iter()
            .skip(expected.len())
            .all(|cell| cell.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> ModelCsvLayout {
        ModelCsvLayout {
            state_count: 2,
            variables: VariableCatalogConfig {
                mvs: vec!["valve".into()],
                cvs: vec!["temperature".into(), "pressure".into()],
                dvs: vec!["feed".into()],
            },
        }
    }

    #[test]
    fn template_labels_every_coefficient_and_signal() {
        let template = layout().template().unwrap();
        assert_eq!(
            template,
            "A,x1,x2\nx1,a11,a12\nx2,a21,a22\n\"\"\nB,valve,feed\nx1,b11,b12\nx2,b21,b22\n\"\"\nC,x1,x2\ntemperature,c11,c12\npressure,c21,c22\n\"\"\nD,valve,feed\ntemperature,d11,d12\npressure,d21,d22\n\"\"\n"
        );
    }

    const NUMERIC: &str = "A,x1,x2\nx1,-1,0\nx2,0,-2\n\nB,valve,feed\nx1,1,2\nx2,3,4\n\nC,x1,x2\ntemperature,5,6\npressure,7,8\n\nD,valve,feed\ntemperature,0,1e-3\npressure,-2.5,0\n";

    #[test]
    fn import_preserves_all_four_matrices_and_mv_dv_order() {
        let model = layout().import(NUMERIC).unwrap();
        assert_eq!(model.a, vec![vec![-1.0, 0.0], vec![0.0, -2.0]]);
        assert_eq!(model.b, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        assert_eq!(model.c, vec![vec![5.0, 6.0], vec![7.0, 8.0]]);
        assert_eq!(model.d, vec![vec![0.0, 0.001], vec![-2.5, 0.0]]);
        assert_eq!(model.u_order, vec!["valve", "feed"]);
        assert_eq!(model.y_order, vec!["temperature", "pressure"]);
    }

    #[test]
    fn import_rejects_partial_stale_and_non_numeric_files() {
        for csv in [
            layout().template().unwrap(),
            NUMERIC.replace("x2,3,4", "x2,3"),
            NUMERIC.replace("valve,feed", "feed,valve"),
            NUMERIC.replace("pressure,7,8", "temperature,7,8"),
            NUMERIC.replace("pressure,-2.5,0", "pressure,NaN,0"),
            NUMERIC.replace("pressure,-2.5,0", "pressure,1e999,0"),
            NUMERIC.replace("pressure,-2.5,0", "pressure,,0"),
            NUMERIC.replace("pressure,-2.5,0", ""),
            format!("{NUMERIC}extra,1,2\n"),
        ] {
            assert!(layout().import(&csv).is_err(), "accepted {csv}");
        }
    }

    #[test]
    fn spreadsheet_padding_quoted_signals_and_zero_width_matrices_are_supported() {
        let layout = ModelCsvLayout {
            state_count: 1,
            variables: VariableCatalogConfig {
                mvs: vec!["valve, main".into()],
                cvs: vec!["temperature\nreactor".into()],
                dvs: vec![],
            },
        };
        let csv = layout
            .template()
            .unwrap()
            .replace("a11", "-1")
            .replace("b11", "1")
            .replace("c11", "1")
            .replace("d11", "0");
        assert_eq!(layout.import(&csv).unwrap().u_order, vec!["valve, main"]);
        let empty = ModelCsvLayout {
            state_count: 1,
            variables: VariableCatalogConfig::default(),
        };
        let padded = "\u{feff}A,x1,\r\nx1,-1,\r\nB,,\r\nx1,,\r\nC,x1,\r\nD,,\r\n";
        let model = empty.import(padded).unwrap();
        assert_eq!(model.b, vec![Vec::<f64>::new()]);
        assert!(model.c.is_empty() && model.d.is_empty());
        assert!(
            ModelCsvLayout {
                state_count: 0,
                variables: VariableCatalogConfig::default()
            }
            .template()
            .is_err()
        );
    }
}
