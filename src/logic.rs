use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{CStr, CString, c_char},
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use libloading::Library;
use serde::{Deserialize, Serialize};
use syn::{FnArg, Item, Pat, Type};

use crate::{
    project_config::{
        CustomParameterSchemaEntry, LogicParamType, LogicParamValue, PControllerPairConfig,
    },
    types::{RuntimeState, SignalValue},
};

#[derive(Debug, Clone, Serialize)]
pub struct LogicMetadata {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterSchemaEntry {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub param_type: LogicParamType,
    pub default: LogicParamValue,
}

pub trait ControlLogic: Send + Sync {
    fn metadata(&self) -> LogicMetadata;
    fn init(
        &mut self,
        initial_values: HashMap<String, SignalValue>,
        params: HashMap<String, LogicParamValue>,
    );
    fn step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        state: RuntimeState,
    ) -> Result<HashMap<String, SignalValue>>;
    fn parameter_schema(&self) -> Vec<ParameterSchemaEntry>;
    fn set_params(&mut self, params: HashMap<String, LogicParamValue>);
    fn current_params(&self) -> HashMap<String, LogicParamValue>;
    fn drain_user_logs(&mut self) -> Vec<LogicUserLog> {
        Vec::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicUserLog {
    pub message: String,
    #[serde(default)]
    pub values: HashMap<String, SignalValue>,
}

#[derive(Debug, Clone)]
pub struct PControllerLogic {
    pairs: Vec<PControllerPairConfig>,
    params: HashMap<String, LogicParamValue>,
    _initial_values: HashMap<String, SignalValue>,
}

impl PControllerLogic {
    pub fn new(
        pairs: Vec<PControllerPairConfig>,
        params: HashMap<String, LogicParamValue>,
    ) -> Self {
        Self {
            pairs,
            params,
            _initial_values: HashMap::new(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.pairs.is_empty()
    }

    fn pair_param_key(pair: &PControllerPairConfig, suffix: &str) -> String {
        format!("p.{}.{}.{}", pair.cv, pair.mv, suffix)
    }

    fn pair_kp(&self, pair: &PControllerPairConfig) -> f64 {
        match self.params.get(&Self::pair_param_key(pair, "kp")) {
            Some(LogicParamValue::F64(v)) => *v,
            _ => pair.kp,
        }
    }

    fn pair_bias(&self, pair: &PControllerPairConfig) -> f64 {
        match self.params.get(&Self::pair_param_key(pair, "bias")) {
            Some(LogicParamValue::F64(v)) => *v,
            _ => pair.bias,
        }
    }

    fn input_bool_or_default_true(inputs: &HashMap<String, SignalValue>, key: &str) -> bool {
        match inputs.get(key) {
            None => true,
            Some(SignalValue::Bool(v)) => *v,
            Some(SignalValue::F64(v)) => *v > 0.5,
        }
    }

    fn allowed_pair_param_type(&self, key: &str) -> Option<LogicParamType> {
        for pair in &self.pairs {
            let kp_key = Self::pair_param_key(pair, "kp");
            if key == kp_key {
                return Some(LogicParamType::F64);
            }
            let bias_key = Self::pair_param_key(pair, "bias");
            if key == bias_key {
                return Some(LogicParamType::F64);
            }
        }
        None
    }
}

impl ControlLogic for PControllerLogic {
    fn metadata(&self) -> LogicMetadata {
        LogicMetadata {
            kind: "p_controller".to_string(),
            description: "P-controller with explicit CV->MV pair mapping".to_string(),
        }
    }

    fn init(
        &mut self,
        initial_values: HashMap<String, SignalValue>,
        params: HashMap<String, LogicParamValue>,
    ) {
        self._initial_values = initial_values;
        self.set_params(params);
    }

    fn step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        _state: RuntimeState,
    ) -> Result<HashMap<String, SignalValue>> {
        let mut outputs = HashMap::new();

        for pair in &self.pairs {
            let cv_soll_key = format!("cv.{}.soll", pair.cv);
            let cv_ist_key = format!("cv.{}.ist", pair.cv);
            let cv_enable_key = format!("cv.{}.enable", pair.cv);
            let mv_enable_key = format!("mv.{}.enable", pair.mv);
            let mv_write_key = format!("mv.{}.write", pair.mv);

            if !Self::input_bool_or_default_true(inputs, &cv_enable_key)
                || !Self::input_bool_or_default_true(inputs, &mv_enable_key)
            {
                continue;
            }

            let Some(soll) = inputs.get(&cv_soll_key).and_then(SignalValue::as_f64) else {
                continue;
            };
            let Some(ist) = inputs.get(&cv_ist_key).and_then(SignalValue::as_f64) else {
                continue;
            };

            let error = soll - ist;
            let u = self.pair_kp(pair) * error + self.pair_bias(pair);
            outputs.insert(mv_write_key, SignalValue::F64(u));
        }

        Ok(outputs)
    }

    fn parameter_schema(&self) -> Vec<ParameterSchemaEntry> {
        let mut out = Vec::new();
        for pair in &self.pairs {
            out.push(ParameterSchemaEntry {
                name: Self::pair_param_key(pair, "kp"),
                description: format!("P gain for pair {} -> {}", pair.cv, pair.mv),
                param_type: LogicParamType::F64,
                default: LogicParamValue::F64(pair.kp),
            });
            out.push(ParameterSchemaEntry {
                name: Self::pair_param_key(pair, "bias"),
                description: format!("Bias for pair {} -> {}", pair.cv, pair.mv),
                param_type: LogicParamType::F64,
                default: LogicParamValue::F64(pair.bias),
            });
        }
        out
    }

    fn set_params(&mut self, params: HashMap<String, LogicParamValue>) {
        for (key, value) in params {
            if key.starts_with("io.") {
                self.params.insert(key, value);
                continue;
            }
            if let Some(expected_type) = self.allowed_pair_param_type(&key) {
                if value.matches_type(expected_type) {
                    self.params.insert(key, value);
                }
            }
        }
    }

    fn current_params(&self) -> HashMap<String, LogicParamValue> {
        let mut out = HashMap::new();
        for pair in &self.pairs {
            let kp_key = Self::pair_param_key(pair, "kp");
            if let Some(value) = self.params.get(&kp_key) {
                out.insert(kp_key, value.clone());
            } else {
                out.insert(kp_key, LogicParamValue::F64(pair.kp));
            }

            let bias_key = Self::pair_param_key(pair, "bias");
            if let Some(value) = self.params.get(&bias_key) {
                out.insert(bias_key, value.clone());
            } else {
                out.insert(bias_key, LogicParamValue::F64(pair.bias));
            }
        }
        for (key, value) in &self.params {
            if key.starts_with("io.") {
                out.insert(key.clone(), value.clone());
            }
        }
        out
    }
}

type MetadataFn = unsafe extern "C" fn() -> *mut c_char;
type ControlFn = unsafe extern "C" fn(*const c_char) -> *mut c_char;
type FreeStringFn = unsafe extern "C" fn(*mut c_char);

#[derive(Debug, Clone, Deserialize)]
struct CustomLibraryMetadata {
    required_inputs: Vec<String>,
    required_outputs: Vec<String>,
    parameter_schema: Vec<CustomParameterSchemaEntry>,
}

#[derive(Debug, Clone)]
pub struct CustomLibraryContract {
    pub required_inputs: Vec<String>,
    pub required_outputs: Vec<String>,
    pub parameter_schema: Vec<CustomParameterSchemaEntry>,
}

#[derive(Debug, Serialize)]
struct CustomCallRequest<'a> {
    state: RuntimeState,
    inputs: &'a HashMap<String, SignalValue>,
    params: &'a HashMap<String, LogicParamValue>,
    input_history: HashMap<String, Vec<SignalValue>>,
    output_history: HashMap<String, Vec<SignalValue>>,
    param_history: HashMap<String, Vec<LogicParamValue>>,
}

#[derive(Debug, Deserialize)]
struct CustomCallResponse {
    outputs: HashMap<String, SignalValue>,
    #[serde(default)]
    logs: Vec<LogicUserLog>,
}

pub struct CustomRustLogic {
    _library: Library,
    control_fn: ControlFn,
    free_fn: FreeStringFn,
    required_outputs: HashSet<String>,
    params: HashMap<String, LogicParamValue>,
    schema: Vec<ParameterSchemaEntry>,
    pending_logs: Vec<LogicUserLog>,
    input_history: HashMap<String, VecDeque<SignalValue>>,
    output_history: HashMap<String, VecDeque<SignalValue>>,
    param_history: HashMap<String, VecDeque<LogicParamValue>>,
    max_history_len: usize,
    _initial_values: HashMap<String, SignalValue>,
}

impl CustomRustLogic {
    pub fn load(
        artifact_path: impl AsRef<Path>,
        params: HashMap<String, LogicParamValue>,
    ) -> Result<Self> {
        // SAFETY: Loading a user-provided dynamic library is inherently unsafe.
        // This API keeps the loaded library alive for as long as function pointers are used.
        let library = unsafe { Library::new(artifact_path.as_ref()) }.with_context(|| {
            format!(
                "failed to load custom library {}",
                artifact_path.as_ref().display()
            )
        })?;

        // SAFETY: Symbol names and function signatures are part of the SmartAPC custom ABI.
        let (metadata_fn, control_fn, free_fn) = unsafe {
            let metadata_fn: libloading::Symbol<'_, MetadataFn> =
                library.get(b"smartapc_metadata_json")?;
            let control_fn: libloading::Symbol<'_, ControlFn> =
                library.get(b"smartapc_control_json")?;
            let free_fn: libloading::Symbol<'_, FreeStringFn> =
                library.get(b"smartapc_string_free")?;
            (*metadata_fn, *control_fn, *free_fn)
        };

        let metadata_json = read_allocated_json(metadata_fn, free_fn)
            .context("failed to read smartapc metadata from custom library")?;
        let metadata: CustomLibraryMetadata = serde_json::from_str(&metadata_json)
            .context("failed to decode custom metadata json")?;

        if metadata.required_outputs.is_empty() {
            bail!("custom strategy metadata has no required outputs");
        }

        let schema = metadata
            .parameter_schema
            .iter()
            .map(|entry| ParameterSchemaEntry {
                name: entry.name.clone(),
                description: if entry.description.trim().is_empty() {
                    format!("Custom user parameter '{}'", entry.name)
                } else {
                    entry.description.clone()
                },
                param_type: entry.param_type,
                default: entry
                    .default
                    .clone()
                    .unwrap_or_else(|| default_param_value(entry.param_type)),
            })
            .collect();

        let mut required_outputs = HashSet::new();
        for output in metadata.required_outputs {
            required_outputs.insert(output);
        }

        let _required_inputs = metadata.required_inputs;

        Ok(Self {
            _library: library,
            control_fn,
            free_fn,
            required_outputs,
            params,
            schema,
            pending_logs: Vec::new(),
            input_history: HashMap::new(),
            output_history: HashMap::new(),
            param_history: HashMap::new(),
            max_history_len: 300,
            _initial_values: HashMap::new(),
        })
    }

    fn push_signal_history(
        history: &mut HashMap<String, VecDeque<SignalValue>>,
        key: &str,
        value: &SignalValue,
        max_len: usize,
    ) {
        let queue = history.entry(key.to_string()).or_default();
        queue.push_back(value.clone());
        while queue.len() > max_len {
            let _ = queue.pop_front();
        }
    }

    fn push_param_history(
        history: &mut HashMap<String, VecDeque<LogicParamValue>>,
        key: &str,
        value: &LogicParamValue,
        max_len: usize,
    ) {
        let queue = history.entry(key.to_string()).or_default();
        queue.push_back(value.clone());
        while queue.len() > max_len {
            let _ = queue.pop_front();
        }
    }

    fn snapshot_signal_history(
        history: &HashMap<String, VecDeque<SignalValue>>,
    ) -> HashMap<String, Vec<SignalValue>> {
        history
            .iter()
            .map(|(key, values)| (key.clone(), values.iter().cloned().collect()))
            .collect()
    }

    fn snapshot_param_history(
        history: &HashMap<String, VecDeque<LogicParamValue>>,
    ) -> HashMap<String, Vec<LogicParamValue>> {
        history
            .iter()
            .map(|(key, values)| (key.clone(), values.iter().cloned().collect()))
            .collect()
    }

    fn schema_type_by_name(&self, key: &str) -> Option<LogicParamType> {
        self.schema
            .iter()
            .find(|entry| entry.name == key)
            .map(|entry| entry.param_type)
    }
}

pub fn inspect_custom_library_contract(
    artifact_path: impl AsRef<Path>,
) -> Result<CustomLibraryContract> {
    // SAFETY: Dynamic library loading is required for custom strategy inspection.
    let library = unsafe { Library::new(artifact_path.as_ref()) }.with_context(|| {
        format!(
            "failed to load custom library {}",
            artifact_path.as_ref().display()
        )
    })?;

    // SAFETY: Symbol names are part of the SmartAPC custom ABI.
    let (metadata_fn, free_fn) = unsafe {
        let metadata_fn: libloading::Symbol<'_, MetadataFn> =
            library.get(b"smartapc_metadata_json")?;
        let free_fn: libloading::Symbol<'_, FreeStringFn> = library.get(b"smartapc_string_free")?;
        (*metadata_fn, *free_fn)
    };

    let metadata_json = read_allocated_json(metadata_fn, free_fn)
        .context("failed to read smartapc metadata from custom library")?;
    let metadata: CustomLibraryMetadata =
        serde_json::from_str(&metadata_json).context("failed to decode custom metadata json")?;

    Ok(CustomLibraryContract {
        required_inputs: metadata.required_inputs,
        required_outputs: metadata.required_outputs,
        parameter_schema: metadata.parameter_schema,
    })
}

impl ControlLogic for CustomRustLogic {
    fn metadata(&self) -> LogicMetadata {
        LogicMetadata {
            kind: "custom_rust".to_string(),
            description: "Project-local custom Rust strategy loaded from cdylib".to_string(),
        }
    }

    fn init(
        &mut self,
        initial_values: HashMap<String, SignalValue>,
        params: HashMap<String, LogicParamValue>,
    ) {
        self._initial_values = initial_values;
        self.pending_logs.clear();
        self.input_history.clear();
        self.output_history.clear();
        self.param_history.clear();
        self.set_params(params);
    }

    fn step(
        &mut self,
        inputs: &HashMap<String, SignalValue>,
        state: RuntimeState,
    ) -> Result<HashMap<String, SignalValue>> {
        let active_params = self.current_params();
        for (key, value) in inputs {
            Self::push_signal_history(&mut self.input_history, key, value, self.max_history_len);
        }
        for (key, value) in &active_params {
            Self::push_param_history(&mut self.param_history, key, value, self.max_history_len);
        }

        let request = CustomCallRequest {
            state,
            inputs,
            params: &active_params,
            input_history: Self::snapshot_signal_history(&self.input_history),
            output_history: Self::snapshot_signal_history(&self.output_history),
            param_history: Self::snapshot_param_history(&self.param_history),
        };
        let request_json =
            serde_json::to_string(&request).context("failed to serialize custom call request")?;
        let request_cstr = CString::new(request_json)
            .map_err(|_| anyhow!("custom call request contained NUL byte"))?;

        // SAFETY: The custom library ABI expects a valid NUL-terminated JSON string pointer.
        let response_ptr = unsafe { (self.control_fn)(request_cstr.as_ptr()) };
        if response_ptr.is_null() {
            bail!("custom strategy returned a null response pointer");
        }

        let response_json = c_string_to_rust(response_ptr, self.free_fn)
            .context("failed to decode custom strategy response json")?;
        let response: CustomCallResponse =
            serde_json::from_str(&response_json).context("invalid custom response json")?;

        for required in &self.required_outputs {
            if !response.outputs.contains_key(required) {
                bail!("custom strategy did not set required output '{}'", required);
            }
        }

        for (key, value) in &response.outputs {
            Self::push_signal_history(&mut self.output_history, key, value, self.max_history_len);
        }
        self.pending_logs = response.logs;

        Ok(response.outputs)
    }

    fn parameter_schema(&self) -> Vec<ParameterSchemaEntry> {
        self.schema.clone()
    }

    fn set_params(&mut self, params: HashMap<String, LogicParamValue>) {
        for (key, value) in params {
            if key.starts_with("io.") {
                self.params.insert(key, value);
                continue;
            }
            if let Some(expected_type) = self.schema_type_by_name(&key) {
                if value.matches_type(expected_type) {
                    self.params.insert(key, value);
                }
            }
        }
    }

    fn current_params(&self) -> HashMap<String, LogicParamValue> {
        let mut out = HashMap::new();
        for entry in &self.schema {
            if let Some(value) = self.params.get(&entry.name) {
                out.insert(entry.name.clone(), value.clone());
            } else {
                out.insert(entry.name.clone(), entry.default.clone());
            }
        }
        for (key, value) in &self.params {
            if key.starts_with("io.") {
                out.insert(key.clone(), value.clone());
            }
        }
        out
    }

    fn drain_user_logs(&mut self) -> Vec<LogicUserLog> {
        std::mem::take(&mut self.pending_logs)
    }
}

fn read_allocated_json(metadata_fn: MetadataFn, free_fn: FreeStringFn) -> Result<String> {
    // SAFETY: ABI contract: metadata_fn returns heap-allocated C string freed by free_fn.
    let ptr = unsafe { metadata_fn() };
    if ptr.is_null() {
        bail!("custom metadata function returned null");
    }
    c_string_to_rust(ptr, free_fn)
}

fn c_string_to_rust(ptr: *mut c_char, free_fn: FreeStringFn) -> Result<String> {
    // SAFETY: ptr is expected to be a valid NUL-terminated string from the plugin.
    let out = unsafe { CStr::from_ptr(ptr as *const c_char) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: free_fn must free pointers allocated by the plugin ABI functions.
    unsafe { free_fn(ptr) };
    Ok(out)
}

pub fn parse_custom_parameter_schema_from_source(
    source: &str,
) -> Result<Vec<CustomParameterSchemaEntry>> {
    let ast = syn::parse_file(source).context("failed to parse rust source")?;
    let mut maybe_control_fn = None;

    for item in ast.items {
        if let Item::Fn(function) = item {
            if function.sig.ident == "control" {
                maybe_control_fn = Some(function);
                break;
            }
        }
    }

    let control_fn = maybe_control_fn.ok_or_else(|| anyhow!("missing `control` function"))?;
    if control_fn.sig.inputs.len() < 2 {
        bail!("`control` must have at least ctx and out parameters");
    }

    let mut schema = Vec::new();
    for (index, arg) in control_fn.sig.inputs.iter().enumerate() {
        if index < 2 {
            continue;
        }

        let FnArg::Typed(arg_typed) = arg else {
            bail!("receiver argument is not allowed in `control`");
        };

        let Pat::Ident(pat_ident) = arg_typed.pat.as_ref() else {
            bail!("custom parameter must be a named identifier");
        };
        let name = pat_ident.ident.to_string();
        if name.trim().is_empty() {
            bail!("custom parameter name must not be empty");
        }

        let param_type = parse_param_type(arg_typed.ty.as_ref())?;
        schema.push(CustomParameterSchemaEntry {
            name: name.clone(),
            param_type,
            default: Some(default_param_value(param_type)),
            description: format!("Custom user parameter '{}'", name),
        });
    }

    Ok(schema)
}

fn parse_param_type(ty: &Type) -> Result<LogicParamType> {
    let Type::Path(type_path) = ty else {
        bail!("unsupported parameter type; only f64/bool are allowed");
    };
    let ident = type_path
        .path
        .segments
        .last()
        .ok_or_else(|| anyhow!("unsupported parameter type"))?
        .ident
        .to_string();
    match ident.as_str() {
        "f64" => Ok(LogicParamType::F64),
        "bool" => Ok(LogicParamType::Bool),
        other => bail!(
            "unsupported parameter type '{}'; only f64/bool are allowed",
            other
        ),
    }
}

pub fn default_param_value(param_type: LogicParamType) -> LogicParamValue {
    match param_type {
        LogicParamType::F64 => LogicParamValue::F64(0.0),
        LogicParamType::Bool => LogicParamValue::Bool(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p_controller_step_computes_output() {
        let mut logic = PControllerLogic::new(
            vec![PControllerPairConfig {
                cv: "y1".to_string(),
                mv: "u1".to_string(),
                kp: 2.0,
                bias: 1.0,
            }],
            HashMap::new(),
        );

        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.soll".to_string(), SignalValue::F64(10.0));
        inputs.insert("cv.y1.ist".to_string(), SignalValue::F64(7.0));

        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        assert_eq!(outputs.get("mv.u1.write"), Some(&SignalValue::F64(7.0)));
    }

    #[test]
    fn p_controller_skips_output_when_disabled() {
        let mut logic = PControllerLogic::new(
            vec![PControllerPairConfig {
                cv: "y1".to_string(),
                mv: "u1".to_string(),
                kp: 1.0,
                bias: 0.0,
            }],
            HashMap::new(),
        );

        let mut inputs = HashMap::new();
        inputs.insert("cv.y1.soll".to_string(), SignalValue::F64(10.0));
        inputs.insert("cv.y1.ist".to_string(), SignalValue::F64(7.0));
        inputs.insert("cv.y1.enable".to_string(), SignalValue::Bool(false));

        let outputs = logic.step(&inputs, RuntimeState::Running).expect("step");
        assert!(!outputs.contains_key("mv.u1.write"));
    }

    #[test]
    fn parser_extracts_custom_parameters() {
        let source = r#"
pub struct SmartApcContext;
pub struct SmartApcOutputs;

pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs, gain: f64, use_enable: bool) {
    let _ = (ctx, out, gain, use_enable);
}
"#;
        let schema = parse_custom_parameter_schema_from_source(source).expect("schema");
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0].name, "gain");
        assert_eq!(schema[0].param_type, LogicParamType::F64);
        assert_eq!(schema[1].name, "use_enable");
        assert_eq!(schema[1].param_type, LogicParamType::Bool);
    }
}
