//! CSV I/O imports stage a complete replacement draft. No project files are
//! touched here; the existing revision-checked configuration save owns persistence.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    project_config::{
        CustomIoDefinition, CvIoDefinition, DvIoDefinition, IoDefinitionConfig, MvIoDefinition,
        VariableCatalogConfig,
    },
    types::SignalType,
};

const COLUMNS: &[&str] = &[
    "group",
    "name",
    "signal_type",
    "read_node",
    "write_node",
    "setpoint_node",
    "ll",
    "hl",
    "ll_node",
    "hl_node",
    "enable_node",
    "band_low",
    "band_high",
    "band_low_node",
    "band_high_node",
    "deadband",
    "deadband_node",
];

#[derive(Debug, Serialize)]
pub(crate) struct IoCsvDraft {
    pub variables: VariableCatalogConfig,
    pub io_definition: IoDefinitionConfig,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Row {
    group: String,
    name: String,
    signal_type: String,
    read_node: String,
    write_node: String,
    setpoint_node: String,
    ll: Option<f64>,
    hl: Option<f64>,
    ll_node: String,
    hl_node: String,
    enable_node: String,
    band_low: Option<f64>,
    band_high: Option<f64>,
    band_low_node: String,
    band_high_node: String,
    deadband: Option<f64>,
    deadband_node: String,
}

fn optional(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) fn import_io_csv(content: &str) -> Result<IoCsvDraft> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(content.trim_start_matches('\u{feff}').as_bytes());
    let headers = reader.headers()?.clone();
    let mut seen = std::collections::HashSet::new();
    for column in &headers {
        ensure!(
            COLUMNS.contains(&column),
            "Unknown I/O CSV column '{column}'"
        );
        ensure!(seen.insert(column), "Duplicate I/O CSV column '{column}'");
    }
    ensure!(
        seen.contains("group") && seen.contains("name"),
        "I/O CSV requires group and name columns"
    );
    let mut draft = IoCsvDraft {
        variables: VariableCatalogConfig::default(),
        io_definition: IoDefinitionConfig::default(),
    };
    for (index, record) in reader.records().enumerate() {
        let number = index + 2;
        let record = record.with_context(|| format!("I/O CSV row {number}"))?;
        if record.iter().all(str::is_empty) {
            continue;
        }
        let row: Row = record
            .deserialize(Some(&headers))
            .with_context(|| format!("I/O CSV row {number}"))?;
        let group = row.group.to_lowercase();
        let applicable: &[&str] = match group.as_str() {
            "mv" => &[
                "read_node",
                "write_node",
                "ll",
                "hl",
                "ll_node",
                "hl_node",
                "enable_node",
            ],
            "cv" => &[
                "read_node",
                "setpoint_node",
                "ll",
                "hl",
                "enable_node",
                "band_low",
                "band_high",
                "band_low_node",
                "band_high_node",
                "deadband",
                "deadband_node",
            ],
            "dv" => &["read_node", "enable_node"],
            "custom" => &["read_node", "write_node"],
            _ => anyhow::bail!("I/O CSV row {number}: group must be mv, cv, dv or custom"),
        };
        // Reject unused populated cells rather than silently discarding addresses or limits.
        for (column, value) in headers.iter().zip(&record) {
            ensure!(
                value.is_empty()
                    || ["group", "name", "signal_type"].contains(&column)
                    || applicable.contains(&column),
                "I/O CSV row {number}: {column} is not used for {group}"
            );
            if column.ends_with("_node") && !value.is_empty() {
                ensure!(
                    value.parse::<async_opcua::types::NodeId>().is_ok(),
                    "I/O CSV row {number}: {column} is not a valid OPC UA Node ID"
                );
            }
        }
        for value in [row.ll, row.hl, row.band_low, row.band_high, row.deadband]
            .into_iter()
            .flatten()
        {
            ensure!(
                value.is_finite(),
                "I/O CSV row {number}: numeric values must be finite"
            );
        }
        let signal_type = match row.signal_type.to_lowercase().as_str() {
            "" | "f64" => SignalType::F64,
            "bool" => SignalType::Bool,
            _ => anyhow::bail!("I/O CSV row {number}: signal_type must be f64 or bool"),
        };
        ensure!(
            group == "custom" || signal_type == SignalType::F64,
            "I/O CSV row {number}: MV/CV/DV values must use f64"
        );
        match group.as_str() {
            "mv" => {
                draft.variables.mvs.push(row.name.clone());
                draft.io_definition.mvs.push(MvIoDefinition {
                    name: row.name,
                    mv_read: row.read_node,
                    mv_write: row.write_node,
                    ll: row.ll,
                    hl: row.hl,
                    ll_opc: optional(row.ll_node),
                    hl_opc: optional(row.hl_node),
                    enable: optional(row.enable_node),
                });
            }
            "cv" => {
                draft.variables.cvs.push(row.name.clone());
                draft.io_definition.cvs.push(CvIoDefinition {
                    name: row.name,
                    process_value: row.read_node,
                    setpoint: row.setpoint_node,
                    ll: row.ll,
                    hl: row.hl,
                    enable: optional(row.enable_node),
                    band_low_value: row.band_low,
                    band_high_value: row.band_high,
                    band_low_opc: optional(row.band_low_node),
                    band_high_opc: optional(row.band_high_node),
                    deadband_value: row.deadband,
                    deadband_opc: optional(row.deadband_node),
                });
            }
            "dv" => {
                draft.variables.dvs.push(row.name.clone());
                draft.io_definition.dvs.push(DvIoDefinition {
                    name: row.name,
                    read: row.read_node,
                    enable: optional(row.enable_node),
                });
            }
            "custom" => draft.io_definition.custom.push(CustomIoDefinition {
                name: row.name,
                signal_type,
                read: optional(row.read_node),
                write: optional(row.write_node),
            }),
            _ => unreachable!(),
        }
    }
    ensure!(
        draft.variables.has_any() || !draft.io_definition.custom.is_empty(),
        "I/O CSV must contain at least one variable"
    );
    draft.variables.validate()?;
    draft.io_definition.validate(&draft.variables)?;
    // Also prove the full directional contract can be built before returning any draft.
    draft
        .io_definition
        .to_runtime_io_and_limits(&draft.variables)?;
    Ok(draft)
}

/// A directly importable example demonstrates every group and custom direction.
/// Addresses illustrate syntax; users replace them with their own hardware nodes.
pub(crate) fn example_io_csv() -> Result<String> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    for row in [
        Row {
            group: "mv".into(),
            name: "valve".into(),
            signal_type: "f64".into(),
            read_node: "ns=2;s=ValveActual".into(),
            write_node: "ns=2;s=ValveCommand".into(),
            ll: Some(0.0),
            hl: Some(100.0),
            ..Row::default()
        },
        Row {
            group: "cv".into(),
            name: "temperature".into(),
            signal_type: "f64".into(),
            read_node: "ns=2;s=Temperature".into(),
            setpoint_node: "ns=2;s=TemperatureTarget".into(),
            ll: Some(0.0),
            hl: Some(200.0),
            band_low: Some(-2.0),
            band_high: Some(2.0),
            ..Row::default()
        },
        Row {
            group: "dv".into(),
            name: "feed".into(),
            signal_type: "f64".into(),
            read_node: "ns=2;s=Feed".into(),
            ..Row::default()
        },
        Row {
            group: "custom".into(),
            name: "interlock".into(),
            signal_type: "bool".into(),
            read_node: "ns=2;s=Interlock".into(),
            ..Row::default()
        },
        Row {
            group: "custom".into(),
            name: "indicator".into(),
            signal_type: "bool".into(),
            write_node: "ns=2;s=Indicator".into(),
            ..Row::default()
        },
        Row {
            group: "custom".into(),
            name: "auxiliary".into(),
            signal_type: "f64".into(),
            read_node: "ns=2;s=Auxiliary".into(),
            write_node: "ns=2;s=Auxiliary".into(),
            ..Row::default()
        },
    ] {
        writer.serialize(row)?;
    }
    Ok(String::from_utf8(writer.into_inner()?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_preserves_directions_types_and_strict_strategy_contract() {
        let draft = import_io_csv(&example_io_csv().unwrap()).unwrap();
        let (io, _) = draft
            .io_definition
            .to_runtime_io_and_limits(&draft.variables)
            .unwrap();
        assert_eq!(draft.io_definition.custom.len(), 3);
        assert!(
            io.inputs
                .iter()
                .any(|p| p.name == "custom.interlock.read" && p.signal_type == SignalType::Bool)
        );
        assert!(
            !io.outputs
                .iter()
                .any(|p| p.name.starts_with("custom.interlock."))
        );
        assert!(
            io.outputs
                .iter()
                .any(|p| p.name == "custom.indicator.write" && p.signal_type == SignalType::Bool)
        );
        assert!(
            !io.inputs
                .iter()
                .any(|p| p.name.starts_with("custom.indicator."))
        );
        for points in [&io.inputs, &io.outputs] {
            assert!(points.iter().any(
                |p| p.name.starts_with("custom.auxiliary.") && p.address == "ns=2;s=Auxiliary"
            ));
        }
        let mut strategy = crate::project_config::CustomRustLogicConfig {
            compatible: true,
            source_path: "source.rs".into(),
            artifact_path: "strategy.so".into(),
            required_inputs: io.inputs.iter().map(|p| p.name.clone()).collect(),
            required_outputs: io.outputs.iter().map(|p| p.name.clone()).collect(),
            ..Default::default()
        };
        assert!(strategy.is_compatible_with(&io));
        strategy
            .required_inputs
            .retain(|name| !name.starts_with("custom."));
        assert!(!strategy.is_compatible_with(&io));
    }

    #[test]
    fn invalid_files_never_return_a_partial_draft() {
        for csv in [
            "group,name,read_node\ncustom,a,ns=2;s=a\ncustom,b,\n",
            "group,name,read_node\ncustom,a,ns=2;s=a\ncustom,a,ns=2;s=b\n",
            "group,name,read_node\nmv,a,ns=2;s=a\ncustom,a,ns=2;s=b\n",
            "group,name,read_node\ncustom,a,not-a-node\n",
            "group,name,signal_type,read_node\ncustom,a,string,ns=2;s=a\n",
            "group,name,ll,hl\ncv,a,2,1\n",
            "group,name,ll\nmv,a,NaN\n",
            "group,name,write_node\ndv,a,ns=2;s=a\n",
            "group,name,unknown\ncustom,a,value\n",
            "group,name,name\ncustom,a,b\n",
            "group,name\n",
        ] {
            assert!(import_io_csv(csv).is_err(), "accepted {csv}");
        }
    }

    #[test]
    fn quoted_nodes_bom_and_old_projects_are_supported() {
        let draft = import_io_csv(
            "\u{feff}group,name,read_node,write_node\ncustom,aux,\"ns=2;s=a,b\",ns=2;s=out\n",
        )
        .unwrap();
        assert_eq!(
            draft.io_definition.custom[0].read.as_deref(),
            Some("ns=2;s=a,b")
        );
        let mut cfg = crate::project_config::ProjectConfig::new("custom_only");
        cfg.io_definition = draft.io_definition;
        cfg.rebuild_runtime_io_from_definition().unwrap();
        cfg.validate().unwrap();
        let roundtrip = crate::project_config::from_toml_str(
            &crate::project_config::to_toml_string(&cfg).unwrap(),
        )
        .unwrap();
        assert_eq!(roundtrip.io_definition.custom.len(), 1);
        let legacy: IoDefinitionConfig = toml::from_str("mvs=[]\ncvs=[]\ndvs=[]\n").unwrap();
        assert!(legacy.custom.is_empty());
    }
}
