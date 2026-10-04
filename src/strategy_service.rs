//! Project-local custom Rust strategy generation and build services.
//! Shared by the native application and web engineering jobs.

use crate::{
    logic::{inspect_custom_library_contract, parse_custom_parameter_schema_from_source},
    project::ProjectService,
    project_config::{LogicKind, LogicParamType},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub success: bool,
    pub compatible: bool,
    pub message: String,
    pub output: String,
}

/// Browser source editing is restricted to files belonging to this project.
/// Canonicalization also rejects symlinks escaping the project directory.
pub fn source_path(service: &ProjectService, name: &str) -> Result<PathBuf> {
    let cfg = service.load_config(name)?;
    let relative = Path::new(&cfg.logic.custom_rust.source_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("custom source must be a relative project-local path");
    }
    let root = service.project_dir(name).canonicalize()?;
    let source = root
        .join(relative)
        .canonicalize()
        .context("custom source not found; generate a template first")?;
    if !source.starts_with(&root) || !source.is_file() {
        bail!("custom source must belong to the selected project");
    }
    Ok(source)
}

pub fn generate_template(
    service: &ProjectService,
    name: &str,
) -> Result<crate::project_config::ProjectConfig> {
    let mut cfg = service.load_config(name)?;
    if cfg.io.outputs.is_empty() {
        bail!("I/O is empty. Complete the I/O definition first.");
    }
    let strategy_root = service.project_dir(name).join("logic/custom_strategy");
    let project_root = service.project_dir(name).canonicalize()?;
    // Check existing ancestors and files before creating directories or reading
    // backups. A project may contain symlinks placed by an external editor.
    for relative in ["Cargo.toml", "src/lib.rs", "src/generated_dispatch.rs"] {
        require_project_path(&project_root, &strategy_root.join(relative))?;
    }
    require_project_path(
        &project_root,
        &service.project_dir(name).join("logic/backups"),
    )?;
    let mut backup_message = String::new();
    // Regeneration preserves the three files it replaces plus the saved project.
    // The compiled artifact is untouched until the user explicitly builds again.
    if strategy_root.join("src/lib.rs").exists() {
        let backup = service
            .project_dir(name)
            .join("logic/backups")
            .join(uuid::Uuid::new_v4().to_string());
        for relative in ["Cargo.toml", "src/lib.rs", "src/generated_dispatch.rs"] {
            let original = strategy_root.join(relative);
            if original.is_file() {
                let saved = backup.join(relative);
                fs::create_dir_all(saved.parent().context("missing backup directory")?)?;
                fs::copy(original, saved)?;
            }
        }
        fs::copy(service.config_path(name), backup.join("project.toml"))?;
        backup_message = format!(
            " Previous source and metadata backed up to {}.",
            backup.strip_prefix(service.project_dir(name))?.display()
        );
    }
    let strategy_name = format!("{}_custom_logic", sanitize_crate_name(name));
    let inputs = cfg
        .io
        .inputs
        .iter()
        .map(|point| point.name.clone())
        .collect::<Vec<_>>();
    let outputs = cfg
        .io
        .outputs
        .iter()
        .map(|point| point.name.clone())
        .collect::<Vec<_>>();
    fs::create_dir_all(strategy_root.join("src"))?;
    fs::write(
        strategy_root.join("Cargo.toml"),
        render_custom_cargo_toml(&strategy_name),
    )?;
    fs::write(
        strategy_root.join("src/lib.rs"),
        render_custom_template(&inputs, &outputs),
    )?;
    fs::write(
        strategy_root.join("src/generated_dispatch.rs"),
        render_generated_dispatch(&[]),
    )?;
    let custom = &mut cfg.logic.custom_rust;
    custom.strategy_name = strategy_name.clone();
    custom.source_path = "logic/custom_strategy/src/lib.rs".into();
    custom.artifact_path = format!(
        "logic/custom_strategy/target/release/{}{}{}",
        dylib_prefix(),
        strategy_name,
        dylib_extension()
    );
    custom.parameter_schema.clear();
    custom.required_inputs = inputs;
    custom.required_outputs = outputs;
    custom.compatible = false;
    custom.last_build_message =
        format!("Template generated. Build and check the strategy.{backup_message}");
    if cfg.logic.kind == LogicKind::CustomRust {
        cfg.logic.kind = LogicKind::PController;
    }
    service.save_config(name, &cfg)?;
    service.load_config(name)
}

/// A missing destination is allowed only when its nearest existing ancestor is
/// inside this project. Existing leaf symlinks are checked as well.
fn require_project_path(root: &Path, destination: &Path) -> Result<()> {
    let mut ancestor = destination;
    while fs::symlink_metadata(ancestor).is_err() {
        ancestor = ancestor.parent().context("missing project path ancestor")?;
    }
    if !ancestor.canonicalize()?.starts_with(root) {
        bail!("custom strategy files must belong to the selected project");
    }
    Ok(())
}

/// Call on a blocking worker: Cargo and dynamic-library inspection can take minutes.
pub fn build_strategy(service: &ProjectService, name: &str) -> Result<BuildReport> {
    let mut cfg = service.load_config(name)?;
    let source = source_path(service, name)?;
    let schema = parse_custom_parameter_schema_from_source(&fs::read_to_string(&source)?)?;
    require_project_path(
        &service.project_dir(name).canonicalize()?,
        &source
            .parent()
            .context("missing source directory")?
            .join("generated_dispatch.rs"),
    )?;
    fs::write(
        source
            .parent()
            .context("missing source directory")?
            .join("generated_dispatch.rs"),
        render_generated_dispatch(&schema),
    )?;
    let manifest = service
        .project_dir(name)
        .join("logic/custom_strategy/Cargo.toml");
    let output = Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&manifest)
        .output()
        .context("unable to start cargo build")?;
    let build_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        cfg.logic.custom_rust.compatible = false;
        cfg.logic.custom_rust.last_build_message = first_line_or(&build_output, "Build failed");
        service.save_config(name, &cfg)?;
        return Ok(BuildReport {
            success: false,
            compatible: false,
            message: "Build failed; review compiler output".into(),
            output: build_output,
        });
    }
    let artifact = service
        .project_dir(name)
        .join(&cfg.logic.custom_rust.artifact_path);
    let contract = inspect_custom_library_contract(&artifact)?;
    let expected_inputs = cfg
        .io
        .inputs
        .iter()
        .map(|point| point.name.clone())
        .collect::<Vec<_>>();
    let expected_outputs = cfg
        .io
        .outputs
        .iter()
        .map(|point| point.name.clone())
        .collect::<Vec<_>>();
    let compatible = schema_signature(&schema) == schema_signature(&contract.parameter_schema)
        && io_signature(&expected_inputs) == io_signature(&contract.required_inputs)
        && io_signature(&expected_outputs) == io_signature(&contract.required_outputs);
    let message = if compatible {
        "Build succeeded; contract is compatible".to_string()
    } else {
        format!(
            "Build succeeded with an incompatible contract: {}",
            summarize_io_mismatch(
                &expected_inputs,
                &expected_outputs,
                &contract.required_inputs,
                &contract.required_outputs
            )
        )
    };
    cfg.logic.custom_rust.parameter_schema = contract.parameter_schema;
    cfg.logic.custom_rust.required_inputs = contract.required_inputs;
    cfg.logic.custom_rust.required_outputs = contract.required_outputs;
    cfg.logic.custom_rust.compatible = compatible;
    cfg.logic.custom_rust.last_build_message = message.clone();
    cfg.sync_active_online_params();
    service.save_config(name, &cfg)?;
    Ok(BuildReport {
        success: true,
        compatible,
        message,
        output: build_output,
    })
}

fn sanitize_crate_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "smartapc_custom_logic".to_string()
    } else {
        out
    }
}

fn dylib_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        ".dll"
    } else if cfg!(target_os = "macos") {
        ".dylib"
    } else {
        ".so"
    }
}

fn dylib_prefix() -> &'static str {
    if cfg!(target_os = "windows") {
        ""
    } else {
        "lib"
    }
}

fn first_line_or(input: &str, fallback: &str) -> String {
    input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn io_signature(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = values.iter().map(|v| v.trim().to_string()).collect();
    out.sort();
    out.dedup();
    out
}

fn schema_signature(values: &[crate::project_config::CustomParameterSchemaEntry]) -> Vec<String> {
    let mut out = values
        .iter()
        .map(|entry| format!("{}:{:?}", entry.name.trim(), entry.param_type))
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

fn summarize_io_mismatch(
    expected_inputs: &[String],
    expected_outputs: &[String],
    actual_inputs: &[String],
    actual_outputs: &[String],
) -> String {
    let exp_in = io_signature(expected_inputs);
    let exp_out = io_signature(expected_outputs);
    let act_in = io_signature(actual_inputs);
    let act_out = io_signature(actual_outputs);

    let missing_in: Vec<_> = exp_in
        .iter()
        .filter(|name| !act_in.contains(name))
        .cloned()
        .collect();
    let extra_in: Vec<_> = act_in
        .iter()
        .filter(|name| !exp_in.contains(name))
        .cloned()
        .collect();
    let missing_out: Vec<_> = exp_out
        .iter()
        .filter(|name| !act_out.contains(name))
        .cloned()
        .collect();
    let extra_out: Vec<_> = act_out
        .iter()
        .filter(|name| !exp_out.contains(name))
        .cloned()
        .collect();

    let mut parts = Vec::new();
    if !missing_in.is_empty() {
        parts.push(format!("missing inputs [{}]", missing_in.join(", ")));
    }
    if !extra_in.is_empty() {
        parts.push(format!("additional inputs [{}]", extra_in.join(", ")));
    }
    if !missing_out.is_empty() {
        parts.push(format!("missing outputs [{}]", missing_out.join(", ")));
    }
    if !extra_out.is_empty() {
        parts.push(format!("additional outputs [{}]", extra_out.join(", ")));
    }

    if parts.is_empty() {
        "unknown mismatch".to_string()
    } else {
        parts.join("; ")
    }
}

pub(crate) fn render_custom_cargo_toml(crate_name: &str) -> String {
    format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#
    )
}

pub(crate) fn render_custom_template(
    required_inputs: &[String],
    required_outputs: &[String],
) -> String {
    let required_inputs_json =
        serde_json::to_string_pretty(required_inputs).unwrap_or_else(|_| "[]".to_string());
    let required_outputs_json =
        serde_json::to_string_pretty(required_outputs).unwrap_or_else(|_| "[]".to_string());

    format!(
        r#"use std::collections::HashMap;
use std::ffi::{{c_char, CStr, CString}};

use serde::{{Deserialize, Serialize}};

mod generated_dispatch;
use generated_dispatch::{{call_control, parameter_schema}};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {{
    Running,
    Hold,
    Alarm,
    Off,
}}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SignalValue {{
    F64(f64),
    Bool(bool),
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {{
    F64(f64),
    Bool(bool),
}}

impl ParamValue {{
    pub fn as_f64(&self) -> Option<f64> {{
        match self {{
            ParamValue::F64(v) => Some(*v),
            _ => None,
        }}
    }}

    pub fn as_bool(&self) -> Option<bool> {{
        match self {{
            ParamValue::Bool(v) => Some(*v),
            _ => None,
        }}
    }}
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartApcContext {{
    // Current runtime state.
    pub state: RuntimeState,
    // Configured controller timestep in seconds.
    pub cycle_time_s: f64,
    // Current input snapshot (all available inputs).
    pub inputs: HashMap<String, SignalValue>,
    // Current online parameters.
    pub params: HashMap<String, ParamValue>,
    // History of inputs (oldest -> newest).
    #[serde(default)]
    pub input_history: HashMap<String, Vec<SignalValue>>,
    // History of outputs (oldest -> newest).
    #[serde(default)]
    pub output_history: HashMap<String, Vec<SignalValue>>,
    // History of online parameters (oldest -> newest).
    #[serde(default)]
    pub param_history: HashMap<String, Vec<ParamValue>>,
}}

#[derive(Debug, Clone, Default)]
pub struct SmartApcOutputs {{
    pub values: HashMap<String, SignalValue>,
    // Additional user logs emitted by the control strategy.
    pub logs: Vec<UserLogEntry>,
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserLogEntry {{
    pub message: String,
    #[serde(default)]
    pub values: HashMap<String, SignalValue>,
}}

impl SmartApcOutputs {{
    pub fn set_f64(&mut self, key: &str, value: f64) {{
        self.values.insert(key.to_string(), SignalValue::F64(value));
    }}

    pub fn set_bool(&mut self, key: &str, value: bool) {{
        self.values.insert(key.to_string(), SignalValue::Bool(value));
    }}

    pub fn log(&mut self, message: impl Into<String>) {{
        self.logs.push(UserLogEntry {{
            message: message.into(),
            values: HashMap::new(),
        }});
    }}

    pub fn log_value(&mut self, message: impl Into<String>, key: &str, value: SignalValue) {{
        let mut values = HashMap::new();
        values.insert(key.to_string(), value);
        self.logs.push(UserLogEntry {{
            message: message.into(),
            values,
        }});
    }}

    pub fn log_values(&mut self, message: impl Into<String>, values: HashMap<String, SignalValue>) {{
        self.logs.push(UserLogEntry {{
            message: message.into(),
            values,
        }});
    }}

    pub fn log_future_prediction_values(&mut self, values: HashMap<String, SignalValue>) {{
        self.log_values("future_prediction", values);
    }}
}}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterSchemaEntry {{
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    pub description: String,
    pub default: ParamValue,
}}

#[derive(Debug, Serialize)]
struct Metadata {{
    required_inputs: Vec<String>,
    required_outputs: Vec<String>,
    parameter_schema: Vec<ParameterSchemaEntry>,
}}

#[derive(Debug, Deserialize)]
struct CallRequest {{
    state: RuntimeState,
    #[serde(default)]
    cycle_time_s: f64,
    inputs: HashMap<String, SignalValue>,
    params: HashMap<String, ParamValue>,
    #[serde(default)]
    input_history: HashMap<String, Vec<SignalValue>>,
    #[serde(default)]
    output_history: HashMap<String, Vec<SignalValue>>,
    #[serde(default)]
    param_history: HashMap<String, Vec<ParamValue>>,
}}

#[derive(Debug, Serialize)]
struct CallResponse {{
    outputs: HashMap<String, SignalValue>,
    logs: Vec<UserLogEntry>,
}}

const REQUIRED_INPUTS: &[&str] = &{required_inputs_json};
const REQUIRED_OUTPUTS: &[&str] = &{required_outputs_json};

pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs) {{
    // -----------------------------------------------------------------------------------------
    // TEMPLATE NOTES
    // 1) ctx.inputs contains the current cycle value per signal; ctx.cycle_time_s is the timestep.
    // 2) ctx.input_history/output_history/param_history contain historical values (oldest -> newest).
    // 3) Additional parameters can be added as function arguments:
    //    for example `pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs, gain: f64)`.
    //    These are automatically managed as online parameters.
    // 4) All required `mv.*.write` outputs must be set.
    // 5) Custom logs can be generated with out.log(...), out.log_value(...), out.log_values(...).
    // 6) To add optional future plots, log F64 values using these keys:
    //    - future_pred_without_control.cv.<cv>.k01
    //    - future_pred_with_control.cv.<cv>.k01
    //    - future_pred_with_control.mv.<mv>.k01
    //    Example: values.insert("future_pred_with_control.cv.y1.k01".into(), SignalValue::F64(y_pred));
    //    Then call out.log_future_prediction_values(values).
    // -----------------------------------------------------------------------------------------
    let _ = ctx;
    // Template default: set every required MV.write output.
    for output_name in REQUIRED_OUTPUTS {{
        out.set_f64(output_name, 0.0);
    }}
}}

#[unsafe(no_mangle)]
pub extern "C" fn smartapc_metadata_json() -> *mut c_char {{
    let metadata = Metadata {{
        required_inputs: REQUIRED_INPUTS.iter().map(|s| (*s).to_string()).collect(),
        required_outputs: REQUIRED_OUTPUTS.iter().map(|s| (*s).to_string()).collect(),
        parameter_schema: parameter_schema(),
    }};

    match serde_json::to_string(&metadata) {{
        Ok(json) => CString::new(json).map(CString::into_raw).unwrap_or(std::ptr::null_mut()),
        Err(_) => std::ptr::null_mut(),
    }}
}}

#[unsafe(no_mangle)]
pub extern "C" fn smartapc_control_json(input: *const c_char) -> *mut c_char {{
    if input.is_null() {{
        return std::ptr::null_mut();
    }}

    let input_str = unsafe {{ CStr::from_ptr(input) }}.to_string_lossy();
    let req: CallRequest = match serde_json::from_str(&input_str) {{
        Ok(v) => v,
        Err(_) => return std::ptr::null_mut(),
    }};

    let ctx = SmartApcContext {{
        state: req.state,
        cycle_time_s: req.cycle_time_s,
        inputs: req.inputs,
        params: req.params,
        input_history: req.input_history,
        output_history: req.output_history,
        param_history: req.param_history,
    }};
    let mut out = SmartApcOutputs::default();
    call_control(&ctx, &mut out);

    let resp = CallResponse {{
        outputs: out.values,
        logs: out.logs,
    }};
    match serde_json::to_string(&resp) {{
        Ok(json) => CString::new(json).map(CString::into_raw).unwrap_or(std::ptr::null_mut()),
        Err(_) => std::ptr::null_mut(),
    }}
}}

#[unsafe(no_mangle)]
pub extern "C" fn smartapc_string_free(ptr: *mut c_char) {{
    if ptr.is_null() {{
        return;
    }}
    unsafe {{
        drop(CString::from_raw(ptr));
    }}
}}
"#
    )
}

pub(crate) fn render_generated_dispatch(
    schema: &[crate::project_config::CustomParameterSchemaEntry],
) -> String {
    let mut call_args = String::new();
    let mut schema_entries = String::new();

    for entry in schema {
        let (extract, type_name, default_expr) = match entry.param_type {
            LogicParamType::F64 => (
                format!(
                    "ctx.params.get(\"{}\").and_then(crate::ParamValue::as_f64).unwrap_or(0.0)",
                    entry.name
                ),
                "f64",
                "crate::ParamValue::F64(0.0)",
            ),
            LogicParamType::Bool => (
                format!(
                    "ctx.params.get(\"{}\").and_then(crate::ParamValue::as_bool).unwrap_or(false)",
                    entry.name
                ),
                "bool",
                "crate::ParamValue::Bool(false)",
            ),
        };
        call_args.push_str(", ");
        call_args.push_str(&extract);
        schema_entries.push_str(&format!(
            "    out.push(crate::ParameterSchemaEntry {{ name: \"{name}\".to_string(), param_type: \"{type_name}\".to_string(), description: \"Custom user parameter '{name}'\".to_string(), default: {default_expr}, }});\n",
            name = entry.name,
            type_name = type_name,
            default_expr = default_expr
        ));
    }

    format!(
        r#"pub fn call_control(ctx: &crate::SmartApcContext, out: &mut crate::SmartApcOutputs) {{
    crate::control(ctx, out{call_args});
}}

pub fn parameter_schema() -> Vec<crate::ParameterSchemaEntry> {{
    let mut out = Vec::new();
{schema_entries}    out
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{project_config::SignalPoint, types::SignalType};

    fn project(service: &ProjectService) {
        let mut cfg = service.create_project("template").unwrap();
        cfg.io.outputs.push(SignalPoint {
            name: "mv.u1.write".into(),
            signal_type: SignalType::F64,
            address: "ns=2;s=u1_write".into(),
            opc_tag: None,
        });
        service.save_config("template", &cfg).unwrap();
    }

    #[test]
    fn regeneration_preserves_source_dispatch_manifest_and_saved_contract() {
        let temporary = tempfile::tempdir().unwrap();
        let service = ProjectService::new(temporary.path());
        project(&service);
        generate_template(&service, "template").unwrap();
        let root = service.project_dir("template");
        fs::write(
            root.join("logic/custom_strategy/src/lib.rs"),
            "// operator source",
        )
        .unwrap();
        let saved_config = fs::read(service.config_path("template")).unwrap();
        let regenerated = generate_template(&service, "template").unwrap();
        let backup = fs::read_dir(root.join("logic/backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(backup.join("src/lib.rs")).unwrap(),
            "// operator source"
        );
        assert_eq!(fs::read(backup.join("project.toml")).unwrap(), saved_config);
        assert!(backup.join("Cargo.toml").is_file());
        assert!(backup.join("src/generated_dispatch.rs").is_file());
        assert!(!regenerated.logic.custom_rust.compatible);
        assert!(
            regenerated
                .logic
                .custom_rust
                .last_build_message
                .contains("backed up")
        );
    }

    #[cfg(unix)]
    #[test]
    fn regeneration_rejects_external_symlink_before_overwriting_or_backing_up() {
        let temporary = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let service = ProjectService::new(temporary.path());
        project(&service);
        generate_template(&service, "template").unwrap();
        let outside = external.path().join("source.rs");
        fs::write(&outside, "// outside source").unwrap();
        let source = service
            .project_dir("template")
            .join("logic/custom_strategy/src/lib.rs");
        fs::remove_file(&source).unwrap();
        std::os::unix::fs::symlink(&outside, &source).unwrap();
        assert!(generate_template(&service, "template").is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "// outside source");
        assert!(
            !service
                .project_dir("template")
                .join("logic/backups")
                .exists()
        );
    }
}
