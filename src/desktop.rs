use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use anyhow::anyhow;
use iced::{
    Element, Length, Subscription, Task, Theme, application, time,
    widget::{
        button, checkbox, column, container, pick_list, row, scrollable, text, text_editor,
        text_input,
    },
};

use crate::{
    communication::{ProtocolAdapter, opcua::OpcUaClientAdapter},
    control_function_tests::{
        ControlFunctionTestReport, DefaultSweepDimension, NumericSweepConfig, SweepExecutionMode,
        SweepSelection, SweepValueKind, default_sweep_dimensions, run_control_function_tests,
    },
    logic::{inspect_custom_library_contract, parse_custom_parameter_schema_from_source},
    project::ProjectService,
    project_config::{
        CommunicationConfig, CvIoDefinition, DvIoDefinition, LinearStateSpaceModelConfig,
        LogicKind, LogicParamType, LogicParamValue, ModelConfig, MvIoDefinition,
        PControllerPairConfig, SidebarTask, SimulationConfig, SimulationSpeedMode,
        VariableCatalogConfig, WatchdogConfig,
    },
    runtime::{ExecutionSnapshot, RuntimeManager},
    storage::SignalSample,
    types::SignalValue,
    web::{AppState, build_router},
};

#[derive(Debug, Clone)]
pub struct DesktopSettings {
    pub project_root: PathBuf,
    pub executable_path: PathBuf,
    pub web_bind: Option<SocketAddr>,
}

pub fn run(settings: DesktopSettings) -> anyhow::Result<()> {
    application(
        move || DesktopApp::boot(settings.clone()),
        DesktopApp::update,
        DesktopApp::view,
    )
    .title(|state: &DesktopApp| {
        if let Some(project) = state.selected_project.as_ref() {
            format!("SmartAPC Desktop - {}", project)
        } else {
            "SmartAPC Desktop".to_string()
        }
    })
    .theme(app_theme)
    .subscription(DesktopApp::subscription)
    .window(iced::window::Settings {
        size: iced::Size::new(1320.0, 900.0),
        ..Default::default()
    })
    .centered()
    .run()
    .map_err(|err| anyhow!("desktop GUI failed: {err}"))
}

fn app_theme(_: &DesktopApp) -> Theme {
    Theme::Light
}

#[derive(Debug, Clone)]
struct VariableNameRow {
    name: String,
}

impl VariableNameRow {
    fn blank() -> Self {
        Self {
            name: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogKindChoice {
    None,
    CounterMirror,
}

const WATCHDOG_KIND_OPTIONS: [WatchdogKindChoice; 2] =
    [WatchdogKindChoice::None, WatchdogKindChoice::CounterMirror];

impl fmt::Display for WatchdogKindChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchdogKindChoice::None => write!(f, "none"),
            WatchdogKindChoice::CounterMirror => write!(f, "counter_mirror"),
        }
    }
}

#[derive(Debug, Clone)]
struct MvIoEditorRow {
    name: String,
    mv_write: String,
    mv_read: String,
    hl_opc: String,
    ll_opc: String,
    hl: String,
    ll: String,
    enable: String,
    read_mv_write: String,
    read_mv_read: String,
    read_hl: String,
    read_ll: String,
    read_enable: String,
    test_write: String,
}

impl MvIoEditorRow {
    fn blank() -> Self {
        Self {
            name: String::new(),
            mv_write: String::new(),
            mv_read: String::new(),
            hl_opc: String::new(),
            ll_opc: String::new(),
            hl: String::new(),
            ll: String::new(),
            enable: String::new(),
            read_mv_write: String::new(),
            read_mv_read: String::new(),
            read_hl: String::new(),
            read_ll: String::new(),
            read_enable: String::new(),
            test_write: String::new(),
        }
    }

    fn from_definition(def: &MvIoDefinition) -> Self {
        Self {
            name: def.name.clone(),
            mv_write: def.mv_write.clone(),
            mv_read: def.mv_read.clone(),
            hl_opc: def.hl_opc.clone().unwrap_or_default(),
            ll_opc: def.ll_opc.clone().unwrap_or_default(),
            hl: def.hl.map(|v| format!("{v}")).unwrap_or_default(),
            ll: def.ll.map(|v| format!("{v}")).unwrap_or_default(),
            enable: def.enable.clone().unwrap_or_default(),
            read_mv_write: String::new(),
            read_mv_read: String::new(),
            read_hl: String::new(),
            read_ll: String::new(),
            read_enable: String::new(),
            test_write: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct CvIoEditorRow {
    name: String,
    soll: String,
    ist: String,
    sollband_value: String,
    sollband_opc: String,
    enable: String,
    read_soll: String,
    read_ist: String,
    read_sollband: String,
    read_enable: String,
    test_write: String,
}

impl CvIoEditorRow {
    fn blank() -> Self {
        Self {
            name: String::new(),
            soll: String::new(),
            ist: String::new(),
            sollband_value: String::new(),
            sollband_opc: String::new(),
            enable: String::new(),
            read_soll: String::new(),
            read_ist: String::new(),
            read_sollband: String::new(),
            read_enable: String::new(),
            test_write: String::new(),
        }
    }

    fn from_definition(def: &CvIoDefinition) -> Self {
        Self {
            name: def.name.clone(),
            soll: def.soll.clone(),
            ist: def.ist.clone(),
            sollband_value: def
                .sollband_value
                .map(|v| format!("{v}"))
                .unwrap_or_default(),
            sollband_opc: def.sollband_opc.clone().unwrap_or_default(),
            enable: def.enable.clone().unwrap_or_default(),
            read_soll: String::new(),
            read_ist: String::new(),
            read_sollband: String::new(),
            read_enable: String::new(),
            test_write: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct DvIoEditorRow {
    name: String,
    read: String,
    enable: String,
    read_read: String,
    read_enable: String,
}

impl DvIoEditorRow {
    fn blank() -> Self {
        Self {
            name: String::new(),
            read: String::new(),
            enable: String::new(),
            read_read: String::new(),
            read_enable: String::new(),
        }
    }

    fn from_definition(def: &DvIoDefinition) -> Self {
        Self {
            name: def.name.clone(),
            read: def.read.clone(),
            enable: def.enable.clone().unwrap_or_default(),
            read_read: String::new(),
            read_enable: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct CommunicationEditorState {
    endpoint_url: String,
    io_cycle_ms: String,
}

impl CommunicationEditorState {
    fn from_config(config: &CommunicationConfig) -> Self {
        Self {
            endpoint_url: config.endpoint_url.clone(),
            io_cycle_ms: config.io_cycle_ms.to_string(),
        }
    }

    fn blank() -> Self {
        Self {
            endpoint_url: "opc.tcp://127.0.0.1:4840".to_string(),
            io_cycle_ms: "1000".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct WatchdogEditorState {
    kind: WatchdogKindChoice,
    read_tag: String,
    write_tag: String,
    mirror_cycle_ms: String,
    stale_timeout_ms: String,
}

impl WatchdogEditorState {
    fn from_config(config: &WatchdogConfig) -> Self {
        match config {
            WatchdogConfig::None => Self {
                kind: WatchdogKindChoice::None,
                read_tag: String::new(),
                write_tag: String::new(),
                mirror_cycle_ms: "1000".to_string(),
                stale_timeout_ms: "2000".to_string(),
            },
            WatchdogConfig::CounterMirror {
                read_node,
                write_node,
                mirror_cycle_ms,
                stale_timeout_ms,
            } => Self {
                kind: WatchdogKindChoice::CounterMirror,
                read_tag: read_node.clone(),
                write_tag: write_node.clone(),
                mirror_cycle_ms: mirror_cycle_ms.to_string(),
                stale_timeout_ms: stale_timeout_ms.to_string(),
            },
        }
    }

    fn blank() -> Self {
        Self::from_config(&WatchdogConfig::None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixKey {
    A,
    B,
    C,
    D,
}

impl MatrixKey {
    fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelKindChoice {
    None,
    LinearStateSpace,
}

const MODEL_KIND_OPTIONS: [ModelKindChoice; 2] =
    [ModelKindChoice::None, ModelKindChoice::LinearStateSpace];

impl fmt::Display for ModelKindChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "No model"),
            Self::LinearStateSpace => write!(f, "Linear state-space"),
        }
    }
}

#[derive(Debug, Clone)]
struct ModelEditorState {
    selected_kind: ModelKindChoice,
    state_count: String,
    a_cells: Vec<Vec<String>>,
    b_cells: Vec<Vec<String>>,
    c_cells: Vec<Vec<String>>,
    d_cells: Vec<Vec<String>>,
    a_import: text_editor::Content,
    b_import: text_editor::Content,
    c_import: text_editor::Content,
    d_import: text_editor::Content,
}

impl ModelEditorState {
    fn from_config(model: Option<&ModelConfig>, vars: &VariableCatalogConfig) -> Self {
        let mut state = Self::blank(vars);

        if let Some(model) = model.and_then(ModelConfig::linear_state_space) {
            state.selected_kind = ModelKindChoice::LinearStateSpace;
            state.state_count = model.state_count.to_string();
            state.a_cells = matrix_to_string_cells(&model.a);
            state.b_cells = matrix_to_string_cells(&model.b);
            state.c_cells = matrix_to_string_cells(&model.c);
            state.d_cells = matrix_to_string_cells(&model.d);
            state.resize_matrices_for(vars);
        }

        state
    }

    fn blank(vars: &VariableCatalogConfig) -> Self {
        let mut state = Self {
            selected_kind: ModelKindChoice::None,
            state_count: "1".to_string(),
            a_cells: Vec::new(),
            b_cells: Vec::new(),
            c_cells: Vec::new(),
            d_cells: Vec::new(),
            a_import: text_editor::Content::new(),
            b_import: text_editor::Content::new(),
            c_import: text_editor::Content::new(),
            d_import: text_editor::Content::new(),
        };
        state.resize_matrices_for(vars);
        state
    }

    fn state_count_value(&self) -> Option<usize> {
        parse_state_count_value(&self.state_count).ok()
    }

    fn resize_matrices_for(&mut self, vars: &VariableCatalogConfig) {
        let Some(state_count) = self.state_count_value() else {
            return;
        };

        let u_count = model_input_order(vars).len();
        let y_count = vars.cvs.len();

        self.a_cells =
            resize_string_matrix(std::mem::take(&mut self.a_cells), state_count, state_count);
        self.b_cells =
            resize_string_matrix(std::mem::take(&mut self.b_cells), state_count, u_count);
        self.c_cells =
            resize_string_matrix(std::mem::take(&mut self.c_cells), y_count, state_count);
        self.d_cells = resize_string_matrix(std::mem::take(&mut self.d_cells), y_count, u_count);
    }

    fn cells_mut(&mut self, matrix: MatrixKey) -> &mut Vec<Vec<String>> {
        match matrix {
            MatrixKey::A => &mut self.a_cells,
            MatrixKey::B => &mut self.b_cells,
            MatrixKey::C => &mut self.c_cells,
            MatrixKey::D => &mut self.d_cells,
        }
    }

    fn cells(&self, matrix: MatrixKey) -> &[Vec<String>] {
        match matrix {
            MatrixKey::A => &self.a_cells,
            MatrixKey::B => &self.b_cells,
            MatrixKey::C => &self.c_cells,
            MatrixKey::D => &self.d_cells,
        }
    }

    fn import_content(&self, matrix: MatrixKey) -> &text_editor::Content {
        match matrix {
            MatrixKey::A => &self.a_import,
            MatrixKey::B => &self.b_import,
            MatrixKey::C => &self.c_import,
            MatrixKey::D => &self.d_import,
        }
    }

    fn import_content_mut(&mut self, matrix: MatrixKey) -> &mut text_editor::Content {
        match matrix {
            MatrixKey::A => &mut self.a_import,
            MatrixKey::B => &mut self.b_import,
            MatrixKey::C => &mut self.c_import,
            MatrixKey::D => &mut self.d_import,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimulationSpeedChoice {
    RealTime,
    X0_1,
    X0_5,
    X10,
    X20,
    X50,
    X100,
    AsFastAsPossible,
}

const SIMULATION_SPEED_OPTIONS: [SimulationSpeedChoice; 8] = [
    SimulationSpeedChoice::RealTime,
    SimulationSpeedChoice::X0_1,
    SimulationSpeedChoice::X0_5,
    SimulationSpeedChoice::X10,
    SimulationSpeedChoice::X20,
    SimulationSpeedChoice::X50,
    SimulationSpeedChoice::X100,
    SimulationSpeedChoice::AsFastAsPossible,
];

impl fmt::Display for SimulationSpeedChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RealTime => write!(f, "real time"),
            Self::X0_1 => write!(f, "0.1x"),
            Self::X0_5 => write!(f, "0.5x"),
            Self::X10 => write!(f, "10x"),
            Self::X20 => write!(f, "20x"),
            Self::X50 => write!(f, "50x"),
            Self::X100 => write!(f, "100x"),
            Self::AsFastAsPossible => write!(f, "as fast as possible"),
        }
    }
}

impl From<SimulationSpeedMode> for SimulationSpeedChoice {
    fn from(value: SimulationSpeedMode) -> Self {
        match value {
            SimulationSpeedMode::RealTime => Self::RealTime,
            SimulationSpeedMode::X0_1 => Self::X0_1,
            SimulationSpeedMode::X0_5 => Self::X0_5,
            SimulationSpeedMode::X10 => Self::X10,
            SimulationSpeedMode::X20 => Self::X20,
            SimulationSpeedMode::X50 => Self::X50,
            SimulationSpeedMode::X100 => Self::X100,
            SimulationSpeedMode::AsFastAsPossible => Self::AsFastAsPossible,
        }
    }
}

impl From<SimulationSpeedChoice> for SimulationSpeedMode {
    fn from(value: SimulationSpeedChoice) -> Self {
        match value {
            SimulationSpeedChoice::RealTime => Self::RealTime,
            SimulationSpeedChoice::X0_1 => Self::X0_1,
            SimulationSpeedChoice::X0_5 => Self::X0_5,
            SimulationSpeedChoice::X10 => Self::X10,
            SimulationSpeedChoice::X20 => Self::X20,
            SimulationSpeedChoice::X50 => Self::X50,
            SimulationSpeedChoice::X100 => Self::X100,
            SimulationSpeedChoice::AsFastAsPossible => Self::AsFastAsPossible,
        }
    }
}

#[derive(Debug, Clone)]
struct SimulationNamedValueRow {
    name: String,
    value: String,
}

impl SimulationNamedValueRow {
    fn from_named_value(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            value: format!("{value}"),
        }
    }
}

#[derive(Debug, Clone)]
struct SimulationEditorState {
    speed: SimulationSpeedChoice,
    cv_setpoints: Vec<SimulationNamedValueRow>,
    cv_initials: Vec<SimulationNamedValueRow>,
    mv_initials: Vec<SimulationNamedValueRow>,
    dv_initials: Vec<SimulationNamedValueRow>,
    cv_noise_std: Vec<SimulationNamedValueRow>,
    mv_noise_std: Vec<SimulationNamedValueRow>,
    dv_disturbances: Vec<SimulationNamedValueRow>,
    cv_offsets: Vec<SimulationNamedValueRow>,
    a_scale_cells: Vec<Vec<String>>,
    b_scale_cells: Vec<Vec<String>>,
}

impl SimulationEditorState {
    fn from_config(
        simulation: &SimulationConfig,
        vars: &VariableCatalogConfig,
        model: Option<&ModelConfig>,
    ) -> Self {
        let mut state = Self::blank(vars, model);
        state.speed = simulation.speed.into();
        state.cv_setpoints = named_value_rows(&vars.cvs, &simulation.cv_setpoints);
        state.cv_initials = named_value_rows(&vars.cvs, &simulation.cv_initials);
        state.mv_initials = named_value_rows(&vars.mvs, &simulation.mv_initials);
        state.dv_initials = named_value_rows(&vars.dvs, &simulation.dv_initials);
        state.cv_noise_std = named_value_rows(&vars.cvs, &simulation.cv_noise_std);
        state.mv_noise_std = named_value_rows(&vars.mvs, &simulation.mv_noise_std);
        state.dv_disturbances = named_value_rows(&vars.dvs, &simulation.dv_disturbances);
        state.cv_offsets = named_value_rows(&vars.cvs, &simulation.cv_offsets);
        state.a_scale_cells = matrix_to_string_cells(&simulation.a_scale_pct);
        state.b_scale_cells = matrix_to_string_cells(&simulation.b_scale_pct);
        state.resize_matrices_for(model);
        state
    }

    fn blank(vars: &VariableCatalogConfig, model: Option<&ModelConfig>) -> Self {
        let mut state = Self {
            speed: SimulationSpeedChoice::RealTime,
            cv_setpoints: named_value_rows(&vars.cvs, &HashMap::new()),
            cv_initials: named_value_rows(&vars.cvs, &HashMap::new()),
            mv_initials: named_value_rows(&vars.mvs, &HashMap::new()),
            dv_initials: named_value_rows(&vars.dvs, &HashMap::new()),
            cv_noise_std: named_value_rows(&vars.cvs, &HashMap::new()),
            mv_noise_std: named_value_rows(&vars.mvs, &HashMap::new()),
            dv_disturbances: named_value_rows(&vars.dvs, &HashMap::new()),
            cv_offsets: named_value_rows(&vars.cvs, &HashMap::new()),
            a_scale_cells: Vec::new(),
            b_scale_cells: Vec::new(),
        };
        state.resize_matrices_for(model);
        state
    }

    fn resize_for(&mut self, vars: &VariableCatalogConfig, model: Option<&ModelConfig>) {
        sync_named_value_rows(&mut self.cv_setpoints, &vars.cvs);
        sync_named_value_rows(&mut self.cv_initials, &vars.cvs);
        sync_named_value_rows(&mut self.mv_initials, &vars.mvs);
        sync_named_value_rows(&mut self.dv_initials, &vars.dvs);
        sync_named_value_rows(&mut self.cv_noise_std, &vars.cvs);
        sync_named_value_rows(&mut self.mv_noise_std, &vars.mvs);
        sync_named_value_rows(&mut self.dv_disturbances, &vars.dvs);
        sync_named_value_rows(&mut self.cv_offsets, &vars.cvs);
        self.resize_matrices_for(model);
    }

    fn resize_matrices_for(&mut self, model: Option<&ModelConfig>) {
        let Some(model) = model.and_then(ModelConfig::linear_state_space) else {
            self.a_scale_cells.clear();
            self.b_scale_cells.clear();
            return;
        };

        self.a_scale_cells = resize_string_matrix(
            std::mem::take(&mut self.a_scale_cells),
            model.state_count,
            model.state_count,
        );
        self.b_scale_cells = resize_string_matrix(
            std::mem::take(&mut self.b_scale_cells),
            model.state_count,
            model.u_order.len(),
        );

        fill_empty_cells(&mut self.a_scale_cells, "100.0");
        fill_empty_cells(&mut self.b_scale_cells, "100.0");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogicKindChoice {
    PController,
    CustomRust,
}

impl fmt::Display for LogicKindChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PController => write!(f, "P-Regler"),
            Self::CustomRust => write!(f, "Eigene Regellogik"),
        }
    }
}

impl From<LogicKind> for LogicKindChoice {
    fn from(value: LogicKind) -> Self {
        match value {
            LogicKind::PController => Self::PController,
            LogicKind::CustomRust => Self::CustomRust,
        }
    }
}

impl From<LogicKindChoice> for LogicKind {
    fn from(value: LogicKindChoice) -> Self {
        match value {
            LogicKindChoice::PController => Self::PController,
            LogicKindChoice::CustomRust => Self::CustomRust,
        }
    }
}

#[derive(Debug, Clone)]
struct PPairEditorRow {
    cv: String,
    mv: String,
    kp: String,
    bias: String,
}

impl PPairEditorRow {
    fn blank() -> Self {
        Self {
            cv: String::new(),
            mv: String::new(),
            kp: "1.0".to_string(),
            bias: "0.0".to_string(),
        }
    }

    fn from_pair(pair: &PControllerPairConfig) -> Self {
        Self {
            cv: pair.cv.clone(),
            mv: pair.mv.clone(),
            kp: pair.kp.to_string(),
            bias: pair.bias.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct LogicEditorState {
    selected_kind: LogicKindChoice,
    p_pairs: Vec<PPairEditorRow>,
    custom_strategy_name: String,
    custom_source_path: String,
    custom_artifact_path: String,
    custom_compatible: bool,
    custom_status: String,
}

impl LogicEditorState {
    fn from_config(
        cfg: &crate::project_config::LogicConfig,
        io: &crate::project_config::IoConfig,
    ) -> Self {
        let p_pairs = if cfg.p_controller.pairs.is_empty() {
            vec![PPairEditorRow::blank()]
        } else {
            cfg.p_controller
                .pairs
                .iter()
                .map(PPairEditorRow::from_pair)
                .collect()
        };
        Self {
            selected_kind: cfg.kind.into(),
            p_pairs,
            custom_strategy_name: cfg.custom_rust.strategy_name.clone(),
            custom_source_path: cfg.custom_rust.source_path.clone(),
            custom_artifact_path: cfg.custom_rust.artifact_path.clone(),
            custom_compatible: cfg.custom_rust.is_compatible_with(io),
            custom_status: cfg.custom_rust.last_build_message.clone(),
        }
    }

    fn blank() -> Self {
        Self {
            selected_kind: LogicKindChoice::PController,
            p_pairs: vec![PPairEditorRow::blank()],
            custom_strategy_name: String::new(),
            custom_source_path: String::new(),
            custom_artifact_path: String::new(),
            custom_compatible: false,
            custom_status: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct ControlFunctionSweepRowState {
    name: String,
    value_kind: SweepValueKind,
    enabled: bool,
    baseline_label: String,
    min: String,
    max: String,
    samples: String,
}

impl ControlFunctionSweepRowState {
    fn from_dimension(dimension: &DefaultSweepDimension) -> Self {
        Self {
            name: dimension.name.clone(),
            value_kind: dimension.value_kind,
            enabled: true,
            baseline_label: dimension.baseline_label.clone(),
            min: dimension
                .default_min
                .map(format_test_numeric)
                .unwrap_or_default(),
            max: dimension
                .default_max
                .map(format_test_numeric)
                .unwrap_or_default(),
            samples: dimension.default_samples.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct ControlFunctionTestState {
    input_rows: Vec<ControlFunctionSweepRowState>,
    param_rows: Vec<ControlFunctionSweepRowState>,
    last_report: Option<ControlFunctionTestReport>,
}

impl ControlFunctionTestState {
    fn from_dimensions(dimensions: &[DefaultSweepDimension]) -> Self {
        let mut input_rows = Vec::new();
        let mut param_rows = Vec::new();

        for dimension in dimensions {
            let row = ControlFunctionSweepRowState::from_dimension(dimension);
            match dimension.kind {
                crate::control_function_tests::SweepDimensionKind::Input => input_rows.push(row),
                crate::control_function_tests::SweepDimensionKind::Parameter => {
                    param_rows.push(row)
                }
            }
        }

        Self {
            input_rows,
            param_rows,
            last_report: None,
        }
    }

    fn blank() -> Self {
        Self {
            input_rows: Vec::new(),
            param_rows: Vec::new(),
            last_report: None,
        }
    }
}

#[derive(Debug, Clone)]
enum WebStatus {
    Disabled,
    Starting(String),
    Running(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupPage {
    Communication,
    Watchdog,
    Io,
    Limits,
    Model,
    Logic,
    Timing,
    Simulation,
    Deployment,
    Params,
    Plots,
}

impl SetupPage {
    fn from_task_id(task_id: &str) -> Option<Self> {
        match task_id {
            "communication" => Some(Self::Communication),
            "watchdog" => Some(Self::Watchdog),
            "io" => Some(Self::Io),
            "limits" => Some(Self::Limits),
            "model" => Some(Self::Model),
            "logic" => Some(Self::Logic),
            "timing" => Some(Self::Timing),
            "simulation" => Some(Self::Simulation),
            "deployment" => Some(Self::Deployment),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Communication => "OPC-UA Kommunikation",
            Self::Watchdog => "Watchdog",
            Self::Io => "Ein- und Ausgänge",
            Self::Limits => "I/O definieren",
            Self::Model => "Define Model",
            Self::Logic => "Regellogik",
            Self::Timing => "Control Function Tests",
            Self::Simulation => "Simulate Controller",
            Self::Deployment => "Deployment",
            Self::Params => "Online Parameter",
            Self::Plots => "Plots",
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectPanelData {
    sidebar_tasks: Vec<SidebarTask>,
    config_params: HashMap<String, LogicParamValue>,
    logic_editor: LogicEditorState,
    model_editor: ModelEditorState,
    simulation_editor: SimulationEditorState,
    control_function_tests: ControlFunctionTestState,
    communication: CommunicationEditorState,
    variables: VariableCatalogConfig,
    mv_names: Vec<VariableNameRow>,
    cv_names: Vec<VariableNameRow>,
    dv_names: Vec<VariableNameRow>,
    mv_io_rows: Vec<MvIoEditorRow>,
    cv_io_rows: Vec<CvIoEditorRow>,
    dv_io_rows: Vec<DvIoEditorRow>,
    watchdog: WatchdogEditorState,
}

#[derive(Debug, Clone)]
struct RefreshData {
    snapshot: Option<ExecutionSnapshot>,
    params: Option<HashMap<String, LogicParamValue>>,
    samples: Vec<SignalSample>,
}

#[derive(Debug, Clone)]
struct ParamApplyResult {
    key: String,
    params: HashMap<String, LogicParamValue>,
}

#[derive(Debug, Clone)]
struct IoReadTestResult {
    mv_values: Vec<MvReadValues>,
    cv_values: Vec<CvReadValues>,
    dv_values: Vec<DvReadValues>,
}

#[derive(Debug, Clone)]
struct MvReadValues {
    mv_write: String,
    mv_read: String,
    hl: String,
    ll: String,
    enable: String,
}

#[derive(Debug, Clone)]
struct CvReadValues {
    soll: String,
    ist: String,
    sollband: String,
    enable: String,
}

#[derive(Debug, Clone)]
struct DvReadValues {
    read: String,
    enable: String,
}

#[derive(Debug, Clone)]
enum SimulationLiveScalarField {
    CvSetpoint(String),
    CvNoiseStd(String),
    MvNoiseStd(String),
    DvDisturbance(String),
    CvOffset(String),
}

#[derive(Debug, Clone)]
enum Message {
    ProjectsLoaded(Result<Vec<String>, String>),
    ProjectSelected(String),
    ProjectPanelLoaded(Result<ProjectPanelData, String>),
    SelectPage(SetupPage),
    NewProjectNameChanged(String),
    CreateProjectPressed,
    ProjectCreated(Result<String, String>),
    StartRuntimePressed,
    StopRuntimePressed,
    RuntimeActionFinished(Result<String, String>),
    Tick,
    Refreshed(Result<RefreshData, String>),
    ParamInputChanged(String, String),
    ApplyParamPressed(String),
    ParamApplied(Result<ParamApplyResult, String>),
    StartWebPressed,
    WebStarted(Result<String, String>),
    OpenPlotsWebPressed,
    PlotsWebOpened(Result<String, String>),
    CommunicationEndpointChanged(String),
    CommunicationCycleChanged(String),
    SaveCommunicationPressed,
    CommunicationSaved(Result<String, String>),
    WatchdogKindChanged(WatchdogKindChoice),
    WatchdogReadTagChanged(String),
    WatchdogWriteTagChanged(String),
    WatchdogCycleChanged(String),
    WatchdogTimeoutChanged(String),
    SaveWatchdogPressed,
    WatchdogSaved(Result<String, String>),
    TestWatchdogPressed,
    WatchdogTested(Result<String, String>),
    AddMvNameRow,
    MvNameChanged(usize, String),
    AddCvNameRow,
    CvNameChanged(usize, String),
    AddDvNameRow,
    DvNameChanged(usize, String),
    SaveVariablesPressed,
    VariablesSaved(Result<String, String>),
    MvWriteChanged(usize, String),
    MvReadChanged(usize, String),
    MvHlOpcChanged(usize, String),
    MvLlOpcChanged(usize, String),
    MvHlChanged(usize, String),
    MvLlChanged(usize, String),
    MvEnableChanged(usize, String),
    MvTestWriteChanged(usize, String),
    CvSollChanged(usize, String),
    CvIstChanged(usize, String),
    CvSollbandValueChanged(usize, String),
    CvSollbandOpcChanged(usize, String),
    CvEnableChanged(usize, String),
    CvTestWriteChanged(usize, String),
    DvReadChanged(usize, String),
    DvEnableChanged(usize, String),
    TestIoDefinitionReadPressed,
    TestIoDefinitionWritePressed,
    IoReadTested(Result<IoReadTestResult, String>),
    IoWriteTested(Result<String, String>),
    SaveIoDefinitionPressed,
    IoDefinitionSaved(Result<String, String>),
    ModelKindChanged(ModelKindChoice),
    ModelStateCountChanged(String),
    ModelCellChanged(MatrixKey, usize, usize, String),
    ModelImportEdited(MatrixKey, text_editor::Action),
    ApplyModelImportPressed(MatrixKey),
    SaveModelPressed,
    ModelSaved(Result<String, String>),
    SimulationSpeedChanged(SimulationSpeedChoice),
    SimulationCvSetpointChanged(usize, String),
    SimulationCvInitialChanged(usize, String),
    SimulationMvInitialChanged(usize, String),
    SimulationDvInitialChanged(usize, String),
    SimulationCvNoiseChanged(usize, String),
    SimulationMvNoiseChanged(usize, String),
    SimulationDvDisturbanceChanged(usize, String),
    SimulationCvOffsetChanged(usize, String),
    SimulationScaleCellChanged(MatrixKey, usize, usize, String),
    SaveSimulationPressed,
    SimulationSaved(Result<String, String>),
    StartSimulationPressed,
    StopSimulationPressed,
    SimulationActionFinished(Result<String, String>),
    SimulationLiveUpdated(Result<String, String>),
    LogicKindChanged(LogicKindChoice),
    AddPControllerPair,
    PPairCvChanged(usize, String),
    PPairMvChanged(usize, String),
    PPairKpChanged(usize, String),
    PPairBiasChanged(usize, String),
    CustomStrategyNameChanged(String),
    CustomSourcePathChanged(String),
    CustomArtifactPathChanged(String),
    SaveLogicPressed,
    LogicSaved(Result<String, String>),
    GenerateLogicTemplatePressed,
    LogicTemplateGenerated(Result<String, String>),
    BuildCustomLogicPressed,
    CustomLogicBuilt(Result<String, String>),
    ControlTestInputEnabledToggled(usize, bool),
    ControlTestInputMinChanged(usize, String),
    ControlTestInputMaxChanged(usize, String),
    ControlTestInputSamplesChanged(usize, String),
    ControlTestParamEnabledToggled(usize, bool),
    ControlTestParamMinChanged(usize, String),
    ControlTestParamMaxChanged(usize, String),
    ControlTestParamSamplesChanged(usize, String),
    RunControlFunctionTestsPressed,
    ControlFunctionTestsFinished(Result<ControlFunctionTestReport, String>),
}

struct DesktopApp {
    project_service: ProjectService,
    runtime: Arc<RuntimeManager>,
    executable_path: PathBuf,
    web_bind: Option<SocketAddr>,
    web_status: WebStatus,
    projects: Vec<String>,
    selected_project: Option<String>,
    sidebar_tasks: Vec<SidebarTask>,
    logic_params: HashMap<String, LogicParamValue>,
    param_inputs: HashMap<String, String>,
    param_dirty_keys: HashSet<String>,
    samples: Vec<SignalSample>,
    active_snapshot: Option<ExecutionSnapshot>,
    selected_page: SetupPage,
    new_project_name: String,
    communication_editor: CommunicationEditorState,
    model_editor: ModelEditorState,
    simulation_editor: SimulationEditorState,
    logic_editor: LogicEditorState,
    control_function_tests: ControlFunctionTestState,
    variables: VariableCatalogConfig,
    mv_name_rows: Vec<VariableNameRow>,
    cv_name_rows: Vec<VariableNameRow>,
    dv_name_rows: Vec<VariableNameRow>,
    mv_io_rows: Vec<MvIoEditorRow>,
    cv_io_rows: Vec<CvIoEditorRow>,
    dv_io_rows: Vec<DvIoEditorRow>,
    watchdog_editor: WatchdogEditorState,
    status_message: String,
    error_message: Option<String>,
}

impl DesktopApp {
    fn boot(settings: DesktopSettings) -> (Self, Task<Message>) {
        let runtime = Arc::new(RuntimeManager::new());
        let project_service = ProjectService::new(&settings.project_root);

        let app = Self {
            project_service: project_service.clone(),
            runtime: Arc::clone(&runtime),
            executable_path: settings.executable_path,
            web_bind: settings.web_bind,
            web_status: if settings.web_bind.is_some() {
                WebStatus::Starting("Web API initialisiert...".to_string())
            } else {
                WebStatus::Disabled
            },
            projects: Vec::new(),
            selected_project: None,
            sidebar_tasks: Vec::new(),
            logic_params: HashMap::new(),
            param_inputs: HashMap::new(),
            param_dirty_keys: HashSet::new(),
            samples: Vec::new(),
            active_snapshot: None,
            selected_page: SetupPage::Communication,
            new_project_name: String::new(),
            communication_editor: CommunicationEditorState::blank(),
            model_editor: ModelEditorState::blank(&VariableCatalogConfig::default()),
            simulation_editor: SimulationEditorState::blank(
                &VariableCatalogConfig::default(),
                None,
            ),
            logic_editor: LogicEditorState::blank(),
            control_function_tests: ControlFunctionTestState::blank(),
            variables: VariableCatalogConfig::default(),
            mv_name_rows: vec![VariableNameRow::blank()],
            cv_name_rows: vec![VariableNameRow::blank()],
            dv_name_rows: vec![VariableNameRow::blank()],
            mv_io_rows: Vec::new(),
            cv_io_rows: Vec::new(),
            dv_io_rows: Vec::new(),
            watchdog_editor: WatchdogEditorState::blank(),
            status_message: format!("Projektordner: {}", settings.project_root.display()),
            error_message: None,
        };

        let mut tasks = vec![
            load_projects_task(project_service.clone()),
            refresh_task(Arc::clone(&runtime)),
        ];

        if settings.web_bind.is_some() {
            tasks.push(Task::done(Message::StartWebPressed));
        }

        (app, Task::batch(tasks))
    }

    fn subscription(&self) -> Subscription<Message> {
        time::every(Duration::from_secs(1)).map(|_| Message::Tick)
    }

    fn execution_active(&self) -> bool {
        self.active_snapshot.is_some()
    }

    fn runtime_running(&self) -> bool {
        matches!(self.active_snapshot, Some(ExecutionSnapshot::Runtime(_)))
    }

    fn simulation_running(&self) -> bool {
        matches!(self.active_snapshot, Some(ExecutionSnapshot::Simulation(_)))
    }

    fn maybe_save_live_simulation_scalar(
        &self,
        field: SimulationLiveScalarField,
        raw: &str,
    ) -> Task<Message> {
        if !self.simulation_running() {
            return Task::none();
        }
        let Some(project_name) = self.selected_project.clone() else {
            return Task::none();
        };
        let parsed = match parse_live_simulation_scalar(&field, raw) {
            Ok(value) => value,
            Err(_) => return Task::none(),
        };
        save_simulation_live_scalar_task(
            Arc::clone(&self.runtime),
            self.project_service.clone(),
            project_name,
            field,
            parsed,
        )
    }

    fn maybe_save_live_simulation_matrix(
        &self,
        matrix: MatrixKey,
        row_index: usize,
        col_index: usize,
        raw: &str,
    ) -> Task<Message> {
        if !self.simulation_running() {
            return Task::none();
        }
        let Some(project_name) = self.selected_project.clone() else {
            return Task::none();
        };
        let parsed = match parse_live_simulation_matrix_value(raw) {
            Ok(value) => value,
            Err(_) => return Task::none(),
        };
        save_simulation_live_matrix_task(
            Arc::clone(&self.runtime),
            self.project_service.clone(),
            project_name,
            matrix,
            row_index,
            col_index,
            parsed,
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ProjectsLoaded(result) => match result {
                Ok(projects) => {
                    self.projects = projects;
                    self.error_message = None;

                    if self.selected_project.is_none() && !self.projects.is_empty() {
                        let selected = self.projects[0].clone();
                        self.selected_project = Some(selected.clone());
                        self.status_message = format!("Projekt '{}' ausgewählt", selected);
                        return load_project_panel_task(self.project_service.clone(), selected);
                    }
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::ProjectSelected(project_name) => {
                self.selected_project = Some(project_name.clone());
                self.status_message = format!("Projekt '{}' ausgewählt", project_name);
                self.error_message = None;
                load_project_panel_task(self.project_service.clone(), project_name)
            }
            Message::ProjectPanelLoaded(result) => match result {
                Ok(panel) => {
                    self.sidebar_tasks = panel.sidebar_tasks;
                    self.communication_editor = panel.communication;
                    self.model_editor = panel.model_editor;
                    self.simulation_editor = panel.simulation_editor;
                    self.logic_editor = panel.logic_editor;
                    self.control_function_tests = panel.control_function_tests;
                    self.variables = panel.variables;
                    self.mv_name_rows = panel.mv_names;
                    self.cv_name_rows = panel.cv_names;
                    self.dv_name_rows = panel.dv_names;
                    self.mv_io_rows = panel.mv_io_rows;
                    self.cv_io_rows = panel.cv_io_rows;
                    self.dv_io_rows = panel.dv_io_rows;
                    self.watchdog_editor = panel.watchdog;
                    self.param_dirty_keys.clear();

                    if !self.execution_active() {
                        self.logic_params = panel.config_params;
                        self.refresh_param_inputs_full();
                    }
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::SelectPage(page) => {
                self.selected_page = page;
                Task::none()
            }
            Message::LogicKindChanged(kind) => {
                self.logic_editor.selected_kind = kind;
                Task::none()
            }
            Message::AddPControllerPair => {
                self.logic_editor.p_pairs.push(PPairEditorRow::blank());
                Task::none()
            }
            Message::PPairCvChanged(index, value) => {
                if let Some(row) = self.logic_editor.p_pairs.get_mut(index) {
                    row.cv = value;
                }
                Task::none()
            }
            Message::PPairMvChanged(index, value) => {
                if let Some(row) = self.logic_editor.p_pairs.get_mut(index) {
                    row.mv = value;
                }
                Task::none()
            }
            Message::PPairKpChanged(index, value) => {
                if let Some(row) = self.logic_editor.p_pairs.get_mut(index) {
                    row.kp = value;
                }
                Task::none()
            }
            Message::PPairBiasChanged(index, value) => {
                if let Some(row) = self.logic_editor.p_pairs.get_mut(index) {
                    row.bias = value;
                }
                Task::none()
            }
            Message::CustomStrategyNameChanged(value) => {
                self.logic_editor.custom_strategy_name = value;
                Task::none()
            }
            Message::CustomSourcePathChanged(value) => {
                self.logic_editor.custom_source_path = value;
                Task::none()
            }
            Message::CustomArtifactPathChanged(value) => {
                self.logic_editor.custom_artifact_path = value;
                Task::none()
            }
            Message::SaveLogicPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für Logik-Änderungen bitte erst stoppen.".to_string(),
                    );
                    return Task::none();
                }
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                save_logic_task(
                    self.project_service.clone(),
                    project_name,
                    self.logic_editor.clone(),
                    self.logic_params.clone(),
                )
            }
            Message::LogicSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::GenerateLogicTemplatePressed => {
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                generate_logic_template_task(self.project_service.clone(), project_name)
            }
            Message::LogicTemplateGenerated(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::BuildCustomLogicPressed => {
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                build_custom_logic_task(self.project_service.clone(), project_name)
            }
            Message::CustomLogicBuilt(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::ControlTestInputEnabledToggled(index, enabled) => {
                if let Some(row) = self.control_function_tests.input_rows.get_mut(index) {
                    row.enabled = enabled;
                }
                Task::none()
            }
            Message::ControlTestInputMinChanged(index, value) => {
                if let Some(row) = self.control_function_tests.input_rows.get_mut(index) {
                    row.min = value;
                }
                Task::none()
            }
            Message::ControlTestInputMaxChanged(index, value) => {
                if let Some(row) = self.control_function_tests.input_rows.get_mut(index) {
                    row.max = value;
                }
                Task::none()
            }
            Message::ControlTestInputSamplesChanged(index, value) => {
                if let Some(row) = self.control_function_tests.input_rows.get_mut(index) {
                    row.samples = value;
                }
                Task::none()
            }
            Message::ControlTestParamEnabledToggled(index, enabled) => {
                if let Some(row) = self.control_function_tests.param_rows.get_mut(index) {
                    row.enabled = enabled;
                }
                Task::none()
            }
            Message::ControlTestParamMinChanged(index, value) => {
                if let Some(row) = self.control_function_tests.param_rows.get_mut(index) {
                    row.min = value;
                }
                Task::none()
            }
            Message::ControlTestParamMaxChanged(index, value) => {
                if let Some(row) = self.control_function_tests.param_rows.get_mut(index) {
                    row.max = value;
                }
                Task::none()
            }
            Message::ControlTestParamSamplesChanged(index, value) => {
                if let Some(row) = self.control_function_tests.param_rows.get_mut(index) {
                    row.samples = value;
                }
                Task::none()
            }
            Message::RunControlFunctionTestsPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für Control Function Tests bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                self.error_message = None;
                run_control_function_tests_task(
                    self.project_service.clone(),
                    project_name,
                    self.control_function_tests.clone(),
                )
            }
            Message::ControlFunctionTestsFinished(result) => match result {
                Ok(report) => {
                    self.status_message = format!(
                        "Control Function Tests abgeschlossen: {} Fälle",
                        report.execution.executed_case_count
                    );
                    self.control_function_tests.last_report = Some(report);
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::NewProjectNameChanged(value) => {
                self.new_project_name = value;
                Task::none()
            }
            Message::CreateProjectPressed => {
                let name = self.new_project_name.trim().to_string();
                if name.is_empty() {
                    self.error_message = Some("Projektname darf nicht leer sein".to_string());
                    return Task::none();
                }
                create_project_task(self.project_service.clone(), name)
            }
            Message::ProjectCreated(result) => match result {
                Ok(project_name) => {
                    self.new_project_name.clear();
                    self.selected_project = Some(project_name.clone());
                    self.status_message = format!("Projekt '{}' erstellt", project_name);
                    self.error_message = None;
                    Task::batch(vec![
                        load_projects_task(self.project_service.clone()),
                        load_project_panel_task(self.project_service.clone(), project_name),
                    ])
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::StartRuntimePressed => {
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                start_runtime_task(
                    Arc::clone(&self.runtime),
                    self.project_service.clone(),
                    project_name,
                )
            }
            Message::StopRuntimePressed => stop_runtime_task(Arc::clone(&self.runtime)),
            Message::RuntimeActionFinished(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    refresh_task(Arc::clone(&self.runtime))
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::Tick => refresh_task(Arc::clone(&self.runtime)),
            Message::Refreshed(result) => match result {
                Ok(data) => {
                    self.active_snapshot = data.snapshot;
                    self.samples = data.samples;
                    if let Some(params) = data.params {
                        self.logic_params = params;
                        self.refresh_param_inputs_preserving_dirty();
                    }
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::ParamInputChanged(key, value) => {
                self.param_dirty_keys.insert(key.clone());
                self.param_inputs.insert(key, value);
                Task::none()
            }
            Message::ApplyParamPressed(key) => {
                let Some(raw_value) = self.param_inputs.get(&key).cloned() else {
                    self.error_message = Some(format!("Kein Eingabewert für Parameter '{}'", key));
                    return Task::none();
                };
                let Some(current) = self.logic_params.get(&key) else {
                    self.error_message = Some(format!("Unbekannter Parameter '{}'", key));
                    return Task::none();
                };
                let parsed = match parse_logic_param_input(current, &raw_value) {
                    Ok(value) => value,
                    Err(err) => {
                        self.error_message = Some(format!(
                            "Ungültiger Wert '{}' für Parameter '{}': {}",
                            raw_value, key, err
                        ));
                        return Task::none();
                    }
                };
                set_param_task(
                    Arc::clone(&self.runtime),
                    self.project_service.clone(),
                    key,
                    parsed,
                )
            }
            Message::ParamApplied(result) => match result {
                Ok(updated) => {
                    self.param_dirty_keys.remove(&updated.key);
                    self.logic_params = updated.params;
                    self.refresh_param_inputs_preserving_dirty();
                    self.status_message = "Parameter übernommen und persistiert".to_string();
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::StartWebPressed => {
                let Some(bind) = self.web_bind else {
                    self.web_status = WebStatus::Disabled;
                    return Task::none();
                };

                self.web_status = WebStatus::Starting(format!("Web API startet auf {}", bind));
                start_web_task(
                    self.project_service.clone(),
                    Arc::clone(&self.runtime),
                    self.executable_path.clone(),
                    bind,
                )
            }
            Message::WebStarted(result) => {
                match result {
                    Ok(msg) => {
                        self.web_status = WebStatus::Running(msg.clone());
                        self.status_message = msg;
                        self.error_message = None;
                    }
                    Err(err) => {
                        self.web_status = WebStatus::Error(err.clone());
                        self.error_message = Some(err);
                    }
                }
                Task::none()
            }
            Message::OpenPlotsWebPressed => {
                let Some(url) = self.plots_web_url() else {
                    self.error_message =
                        Some("Web API ist deaktiviert. Desktop ohne --no-web starten.".to_string());
                    return Task::none();
                };
                open_url_task(url)
            }
            Message::PlotsWebOpened(result) => match result {
                Ok(msg) => {
                    self.status_message = msg;
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::CommunicationEndpointChanged(value) => {
                self.communication_editor.endpoint_url = value;
                Task::none()
            }
            Message::CommunicationCycleChanged(value) => {
                self.communication_editor.io_cycle_ms = value;
                Task::none()
            }
            Message::SaveCommunicationPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für Kommunikations-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_communication_task(
                    self.project_service.clone(),
                    project_name,
                    self.communication_editor.clone(),
                )
            }
            Message::CommunicationSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::WatchdogKindChanged(kind) => {
                self.watchdog_editor.kind = kind;
                Task::none()
            }
            Message::WatchdogReadTagChanged(value) => {
                self.watchdog_editor.read_tag = value;
                Task::none()
            }
            Message::WatchdogWriteTagChanged(value) => {
                self.watchdog_editor.write_tag = value;
                Task::none()
            }
            Message::WatchdogCycleChanged(value) => {
                self.watchdog_editor.mirror_cycle_ms = value;
                Task::none()
            }
            Message::WatchdogTimeoutChanged(value) => {
                self.watchdog_editor.stale_timeout_ms = value;
                Task::none()
            }
            Message::SaveWatchdogPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für strukturelle Konfig-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_watchdog_task(
                    self.project_service.clone(),
                    project_name,
                    self.watchdog_editor.clone(),
                )
            }
            Message::TestWatchdogPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für den Watchdog-Test bitte erst stoppen.".to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                test_watchdog_task(
                    self.project_service.clone(),
                    project_name,
                    self.watchdog_editor.clone(),
                )
            }
            Message::WatchdogSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::WatchdogTested(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::AddMvNameRow => {
                self.mv_name_rows.push(VariableNameRow::blank());
                self.sync_io_rows_from_name_rows();
                Task::none()
            }
            Message::MvNameChanged(index, value) => {
                if let Some(row) = self.mv_name_rows.get_mut(index) {
                    row.name = value;
                }
                self.sync_io_rows_from_name_rows();
                Task::none()
            }
            Message::AddCvNameRow => {
                self.cv_name_rows.push(VariableNameRow::blank());
                self.sync_io_rows_from_name_rows();
                Task::none()
            }
            Message::CvNameChanged(index, value) => {
                if let Some(row) = self.cv_name_rows.get_mut(index) {
                    row.name = value;
                }
                self.sync_io_rows_from_name_rows();
                Task::none()
            }
            Message::AddDvNameRow => {
                self.dv_name_rows.push(VariableNameRow::blank());
                self.sync_io_rows_from_name_rows();
                Task::none()
            }
            Message::DvNameChanged(index, value) => {
                if let Some(row) = self.dv_name_rows.get_mut(index) {
                    row.name = value;
                }
                self.sync_io_rows_from_name_rows();
                Task::none()
            }
            Message::SaveVariablesPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für strukturelle Konfig-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_variables_task(
                    self.project_service.clone(),
                    project_name,
                    self.mv_name_rows.clone(),
                    self.cv_name_rows.clone(),
                    self.dv_name_rows.clone(),
                )
            }
            Message::VariablesSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::MvWriteChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.mv_write = value;
                    row.read_mv_write.clear();
                }
                Task::none()
            }
            Message::MvReadChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.mv_read = value;
                    row.read_mv_read.clear();
                }
                Task::none()
            }
            Message::MvHlOpcChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.hl_opc = value;
                }
                Task::none()
            }
            Message::MvLlOpcChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.ll_opc = value;
                }
                Task::none()
            }
            Message::MvHlChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.hl = value;
                }
                Task::none()
            }
            Message::MvLlChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.ll = value;
                }
                Task::none()
            }
            Message::MvEnableChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.enable = value;
                }
                Task::none()
            }
            Message::MvTestWriteChanged(index, value) => {
                if let Some(row) = self.mv_io_rows.get_mut(index) {
                    row.test_write = value;
                }
                Task::none()
            }
            Message::CvSollChanged(index, value) => {
                if let Some(row) = self.cv_io_rows.get_mut(index) {
                    row.soll = value;
                    row.read_soll.clear();
                }
                Task::none()
            }
            Message::CvIstChanged(index, value) => {
                if let Some(row) = self.cv_io_rows.get_mut(index) {
                    row.ist = value;
                    row.read_ist.clear();
                }
                Task::none()
            }
            Message::CvSollbandValueChanged(index, value) => {
                if let Some(row) = self.cv_io_rows.get_mut(index) {
                    row.sollband_value = value;
                }
                Task::none()
            }
            Message::CvSollbandOpcChanged(index, value) => {
                if let Some(row) = self.cv_io_rows.get_mut(index) {
                    row.sollband_opc = value;
                }
                Task::none()
            }
            Message::CvEnableChanged(index, value) => {
                if let Some(row) = self.cv_io_rows.get_mut(index) {
                    row.enable = value;
                }
                Task::none()
            }
            Message::CvTestWriteChanged(index, value) => {
                if let Some(row) = self.cv_io_rows.get_mut(index) {
                    row.test_write = value;
                }
                Task::none()
            }
            Message::DvReadChanged(index, value) => {
                if let Some(row) = self.dv_io_rows.get_mut(index) {
                    row.read = value;
                    row.read_read.clear();
                }
                Task::none()
            }
            Message::DvEnableChanged(index, value) => {
                if let Some(row) = self.dv_io_rows.get_mut(index) {
                    row.enable = value;
                }
                Task::none()
            }
            Message::TestIoDefinitionReadPressed => {
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                let endpoint_url = self.communication_editor.endpoint_url.clone();
                test_io_read_task(
                    self.project_service.clone(),
                    project_name,
                    endpoint_url,
                    self.mv_io_rows.clone(),
                    self.cv_io_rows.clone(),
                    self.dv_io_rows.clone(),
                )
            }
            Message::TestIoDefinitionWritePressed => {
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                test_io_write_all_task(
                    self.project_service.clone(),
                    project_name,
                    self.communication_editor.endpoint_url.clone(),
                    self.mv_io_rows.clone(),
                    self.cv_io_rows.clone(),
                )
            }
            Message::IoReadTested(result) => match result {
                Ok(values) => {
                    for (row, value) in self.mv_io_rows.iter_mut().zip(values.mv_values) {
                        row.read_mv_write = value.mv_write;
                        row.read_mv_read = value.mv_read;
                        row.read_hl = value.hl;
                        row.read_ll = value.ll;
                        row.read_enable = value.enable;
                    }
                    for (row, value) in self.cv_io_rows.iter_mut().zip(values.cv_values) {
                        row.read_soll = value.soll;
                        row.read_ist = value.ist;
                        row.read_sollband = value.sollband;
                        row.read_enable = value.enable;
                    }
                    for (row, value) in self.dv_io_rows.iter_mut().zip(values.dv_values) {
                        row.read_read = value.read;
                        row.read_enable = value.enable;
                    }
                    self.status_message = "I/O OPC-UA Read-Test abgeschlossen".to_string();
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::IoWriteTested(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::SaveIoDefinitionPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für strukturelle Konfig-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_io_definition_task(
                    self.project_service.clone(),
                    project_name,
                    self.mv_name_rows.clone(),
                    self.cv_name_rows.clone(),
                    self.dv_name_rows.clone(),
                    self.mv_io_rows.clone(),
                    self.cv_io_rows.clone(),
                    self.dv_io_rows.clone(),
                )
            }
            Message::IoDefinitionSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::ModelKindChanged(kind) => {
                self.model_editor.selected_kind = kind;
                if kind == ModelKindChoice::LinearStateSpace
                    && self.model_editor.state_count_value().is_none()
                {
                    self.model_editor.state_count = "1".to_string();
                }
                self.model_editor.resize_matrices_for(&self.variables);
                self.simulation_editor.resize_for(
                    &self.variables,
                    build_model_preview(&self.model_editor, &self.variables).as_ref(),
                );
                Task::none()
            }
            Message::ModelStateCountChanged(value) => {
                self.model_editor.state_count = value;
                self.model_editor.resize_matrices_for(&self.variables);
                self.simulation_editor.resize_for(
                    &self.variables,
                    build_model_preview(&self.model_editor, &self.variables).as_ref(),
                );
                Task::none()
            }
            Message::ModelCellChanged(matrix, row_index, col_index, value) => {
                if let Some(row) = self.model_editor.cells_mut(matrix).get_mut(row_index) {
                    if let Some(cell) = row.get_mut(col_index) {
                        *cell = value;
                    }
                }
                Task::none()
            }
            Message::ModelImportEdited(matrix, action) => {
                self.model_editor.import_content_mut(matrix).perform(action);
                Task::none()
            }
            Message::ApplyModelImportPressed(matrix) => {
                let rows = self.model_editor.cells(matrix).len();
                let cols = self.model_editor.cells(matrix).first().map_or(0, Vec::len);
                let raw = self.model_editor.import_content(matrix).text();
                match parse_matrix_import_text(&raw, rows, cols) {
                    Ok(parsed) => {
                        *self.model_editor.cells_mut(matrix) = parsed;
                        *self.model_editor.import_content_mut(matrix) = text_editor::Content::new();
                        self.status_message = format!("{} matrix import applied", matrix.label());
                        self.error_message = None;
                    }
                    Err(err) => {
                        self.error_message =
                            Some(format!("{} matrix import failed: {}", matrix.label(), err));
                    }
                }
                Task::none()
            }
            Message::SaveModelPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für Modell-Änderungen bitte erst stoppen.".to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_model_task(
                    self.project_service.clone(),
                    project_name,
                    self.model_editor.clone(),
                )
            }
            Message::ModelSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::SimulationSpeedChanged(speed) => {
                self.simulation_editor.speed = speed;
                Task::none()
            }
            Message::SimulationCvSetpointChanged(index, value) => {
                if let Some(row) = self.simulation_editor.cv_setpoints.get_mut(index) {
                    row.value = value.clone();
                    let name = row.name.clone();
                    return self.maybe_save_live_simulation_scalar(
                        SimulationLiveScalarField::CvSetpoint(name),
                        &value,
                    );
                }
                Task::none()
            }
            Message::SimulationCvInitialChanged(index, value) => {
                if let Some(row) = self.simulation_editor.cv_initials.get_mut(index) {
                    row.value = value;
                }
                Task::none()
            }
            Message::SimulationMvInitialChanged(index, value) => {
                if let Some(row) = self.simulation_editor.mv_initials.get_mut(index) {
                    row.value = value;
                }
                Task::none()
            }
            Message::SimulationDvInitialChanged(index, value) => {
                if let Some(row) = self.simulation_editor.dv_initials.get_mut(index) {
                    row.value = value;
                }
                Task::none()
            }
            Message::SimulationCvNoiseChanged(index, value) => {
                if let Some(row) = self.simulation_editor.cv_noise_std.get_mut(index) {
                    row.value = value.clone();
                    let name = row.name.clone();
                    return self.maybe_save_live_simulation_scalar(
                        SimulationLiveScalarField::CvNoiseStd(name),
                        &value,
                    );
                }
                Task::none()
            }
            Message::SimulationMvNoiseChanged(index, value) => {
                if let Some(row) = self.simulation_editor.mv_noise_std.get_mut(index) {
                    row.value = value.clone();
                    let name = row.name.clone();
                    return self.maybe_save_live_simulation_scalar(
                        SimulationLiveScalarField::MvNoiseStd(name),
                        &value,
                    );
                }
                Task::none()
            }
            Message::SimulationDvDisturbanceChanged(index, value) => {
                if let Some(row) = self.simulation_editor.dv_disturbances.get_mut(index) {
                    row.value = value.clone();
                    let name = row.name.clone();
                    return self.maybe_save_live_simulation_scalar(
                        SimulationLiveScalarField::DvDisturbance(name),
                        &value,
                    );
                }
                Task::none()
            }
            Message::SimulationCvOffsetChanged(index, value) => {
                if let Some(row) = self.simulation_editor.cv_offsets.get_mut(index) {
                    row.value = value.clone();
                    let name = row.name.clone();
                    return self.maybe_save_live_simulation_scalar(
                        SimulationLiveScalarField::CvOffset(name),
                        &value,
                    );
                }
                Task::none()
            }
            Message::SimulationScaleCellChanged(matrix, row_index, col_index, value) => {
                let cells = match matrix {
                    MatrixKey::A => &mut self.simulation_editor.a_scale_cells,
                    MatrixKey::B => &mut self.simulation_editor.b_scale_cells,
                    MatrixKey::C | MatrixKey::D => return Task::none(),
                };
                if let Some(row) = cells.get_mut(row_index) {
                    if let Some(cell) = row.get_mut(col_index) {
                        *cell = value.clone();
                        return self.maybe_save_live_simulation_matrix(
                            matrix, row_index, col_index, &value,
                        );
                    }
                }
                Task::none()
            }
            Message::SaveSimulationPressed => {
                if self.execution_active() {
                    self.error_message = Some(
                        "Ausführung läuft. Für strukturelle Simulations-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                save_simulation_task(
                    self.project_service.clone(),
                    project_name,
                    self.simulation_editor.clone(),
                )
            }
            Message::SimulationSaved(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    if let Some(project_name) = self.selected_project.clone() {
                        load_project_panel_task(self.project_service.clone(), project_name)
                    } else {
                        Task::none()
                    }
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::StartSimulationPressed => {
                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };
                start_simulation_task(
                    Arc::clone(&self.runtime),
                    self.project_service.clone(),
                    project_name,
                )
            }
            Message::StopSimulationPressed => stop_simulation_task(Arc::clone(&self.runtime)),
            Message::SimulationActionFinished(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    refresh_task(Arc::clone(&self.runtime))
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
            Message::SimulationLiveUpdated(result) => match result {
                Ok(status) => {
                    self.status_message = status;
                    self.error_message = None;
                    Task::none()
                }
                Err(err) => {
                    self.error_message = Some(err);
                    Task::none()
                }
            },
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let header = self.header_view();
        let runtime = self.runtime_view();
        let sidebar = self.sidebar_view();
        let page = self.current_page_view();
        let status = self.status_view();

        let main = column![
            header,
            runtime,
            row![sidebar, container(page).width(Length::Fill)]
                .spacing(16)
                .height(Length::Fill),
            status
        ]
        .spacing(16)
        .padding(16)
        .height(Length::Fill);

        container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn header_view(&self) -> Element<'_, Message> {
        let project_selector = pick_list(
            self.projects.clone(),
            self.selected_project.clone(),
            Message::ProjectSelected,
        )
        .placeholder("Projekt wählen")
        .width(Length::Fixed(260.0));

        let create_button = if self.new_project_name.trim().is_empty() {
            button("Projekt erstellen")
        } else {
            button("Projekt erstellen").on_press(Message::CreateProjectPressed)
        };

        let web_button = match self.web_status {
            WebStatus::Disabled | WebStatus::Running(_) | WebStatus::Starting(_) => {
                button("Web API läuft")
            }
            WebStatus::Error(_) => button("Web API neu starten").on_press(Message::StartWebPressed),
        };

        let web_hint = match &self.web_status {
            WebStatus::Disabled => "Web API deaktiviert".to_string(),
            WebStatus::Starting(msg) => msg.clone(),
            WebStatus::Running(msg) => msg.clone(),
            WebStatus::Error(msg) => format!("Web API Fehler: {}", msg),
        };

        container(
            row![
                text("Projekt").width(Length::Fixed(56.0)),
                project_selector,
                text_input("Neuer Projektname", &self.new_project_name)
                    .on_input(Message::NewProjectNameChanged)
                    .width(Length::Fixed(220.0)),
                create_button,
                web_button,
                text(web_hint).size(14)
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
        )
        .padding(12)
        .into()
    }

    fn runtime_view(&self) -> Element<'_, Message> {
        let runtime_running = self.runtime_running();
        let execution_active = self.execution_active();
        let start_button = if self.selected_project.is_some() && !execution_active {
            button("Runtime Start").on_press(Message::StartRuntimePressed)
        } else {
            button("Runtime Start")
        };

        let stop_button = if runtime_running {
            button("Runtime Stop").on_press(Message::StopRuntimePressed)
        } else {
            button("Runtime Stop")
        };

        let summary = match &self.active_snapshot {
            Some(ExecutionSnapshot::Runtime(snapshot)) => format!(
                "Runtime | Projekt={} | State={:?} | Watchdog={:?} | Cycles={} | DeadlineMiss={} | LastDuration={}us | LastJitter={}us",
                snapshot.project_name,
                snapshot.state,
                snapshot.watchdog_status,
                snapshot.metrics.total_cycles,
                snapshot.metrics.deadline_misses,
                snapshot.metrics.last_duration_us,
                snapshot.metrics.last_jitter_us
            ),
            Some(ExecutionSnapshot::Simulation(snapshot)) => format!(
                "Simulation | Projekt={} | State={:?} | t={:.1}s | Cycles={} | LastDuration={}us",
                snapshot.project_name,
                snapshot.state,
                snapshot.simulation_time_s,
                snapshot.metrics.total_cycles,
                snapshot.metrics.last_duration_us
            ),
            None => "Keine aktive Runtime/Simulation".to_string(),
        };

        container(row![start_button, stop_button, text(summary).size(14)].spacing(12))
            .padding(12)
            .into()
    }

    fn sidebar_view(&self) -> Element<'_, Message> {
        let mut content = column![text("Setup Aufgaben").size(20)].spacing(8);
        if self.sidebar_tasks.is_empty() {
            content = content.push(text("Kein Projekt geladen."));
        } else {
            for task in &self.sidebar_tasks {
                if task.id == "params" {
                    continue;
                }
                let marker = if task.configured { "[x]" } else { "[ ]" };
                let label = format!("{} {}", marker, task.label);
                if let Some(page) = SetupPage::from_task_id(&task.id) {
                    let prefix = if self.selected_page == page { ">" } else { " " };
                    content = content.push(
                        button(text(format!("{} {}", prefix, label)))
                            .on_press(Message::SelectPage(page)),
                    );
                } else {
                    content = content.push(text(format!("  {}", label)).size(14));
                }
            }
        }

        let params_prefix = if self.selected_page == SetupPage::Params {
            ">"
        } else {
            " "
        };
        let plots_prefix = if self.selected_page == SetupPage::Plots {
            ">"
        } else {
            " "
        };

        content = content
            .push(text(" "))
            .push(
                button(text(format!(
                    "{params_prefix} {}",
                    SetupPage::Params.title()
                )))
                .on_press(Message::SelectPage(SetupPage::Params)),
            )
            .push(
                button(text(format!("{plots_prefix} {}", SetupPage::Plots.title())))
                    .on_press(Message::SelectPage(SetupPage::Plots)),
            );

        container(scrollable(content))
            .padding(12)
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .into()
    }

    fn current_page_view(&self) -> Element<'_, Message> {
        let execution_active = self.execution_active();
        match self.selected_page {
            SetupPage::Communication => self.communication_editor_view(execution_active),
            SetupPage::Watchdog => self.watchdog_editor_view(execution_active),
            SetupPage::Io => self.io_editor_view(execution_active),
            SetupPage::Limits => self.limits_editor_view(execution_active),
            SetupPage::Model => self.model_editor_view(execution_active),
            SetupPage::Logic => self.logic_editor_view(execution_active),
            SetupPage::Timing => self.control_function_tests_view(execution_active),
            SetupPage::Simulation => self.simulation_editor_view(),
            SetupPage::Deployment => self.placeholder_page_view(
                SetupPage::Deployment,
                "Hier folgt ein Deployment-Assistent (Install/Start/Stop) für Windows-Service-Betrieb.",
            ),
            SetupPage::Params => self.params_view(),
            SetupPage::Plots => self.plots_view(),
        }
    }

    fn placeholder_page_view(
        &self,
        page: SetupPage,
        description: &'static str,
    ) -> Element<'_, Message> {
        container(
            column![
                text(page.title()).size(24),
                text(description).size(15),
                text("Status: Platzhalterseite, noch keine Eingabefelder.").size(14)
            ]
            .spacing(12),
        )
        .padding(12)
        .into()
    }

    fn model_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let has_variables = self.variables.has_any();
        let can_save = self.selected_project.is_some()
            && !runtime_running
            && (self.model_editor.selected_kind == ModelKindChoice::None || has_variables);
        let input_order = model_input_order(&self.variables);
        let output_order = self.variables.cvs.clone();
        let state_count = self.model_editor.state_count_value().unwrap_or(1);
        let dimensions = format!(
            "nx = {} | nu = {} | ny = {}",
            state_count,
            input_order.len(),
            output_order.len()
        );
        let state_count_input = if self.model_editor.selected_kind
            == ModelKindChoice::LinearStateSpace
            && has_variables
        {
            text_input("Number of states", &self.model_editor.state_count)
                .on_input(Message::ModelStateCountChanged)
                .width(Length::Fixed(180.0))
        } else {
            text_input("Number of states", &self.model_editor.state_count)
                .width(Length::Fixed(180.0))
        };

        let mut content = column![
            text("Define Model").size(24),
            text("Optional continuous-time state-space setup. The configured model is stored in project.toml and is used by the offline controller simulation.")
                .size(14),
            row![
                text("Model type").width(Length::Fixed(120.0)),
                pick_list(
                    MODEL_KIND_OPTIONS,
                    Some(self.model_editor.selected_kind),
                    Message::ModelKindChanged,
                )
                .width(Length::Fixed(220.0))
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(12);

        if runtime_running {
            content = content.push(
                text("Runtime oder Simulation läuft: erst stoppen, dann Modell ändern.").size(13),
            );
        }

        if self.model_editor.selected_kind == ModelKindChoice::None {
            content = content.push(
                container(
                    text("No model is stored for this project. Switch to \"Linear state-space\" to configure A, B, C, and D.")
                        .size(14),
                )
                .padding(12),
            );
        } else if !has_variables {
            content = content.push(
                container(
                    column![
                        text("Define MV/CV/DV names before configuring the model.").size(16),
                        text("The state-space editor needs the current MV, DV, and CV order to size B, C, and D correctly.")
                            .size(14)
                    ]
                    .spacing(8),
                )
                .padding(12),
            );
        } else {
            let explanation = container(
                column![
                    text("Dynamics").size(18),
                    text("x_dot = A x + B u").size(16),
                    text("y = C x + D u").size(16),
                    text(format!("x = [{}]^T", format_state_vector(state_count))).size(14),
                    text(format!("u = [{}]^T", format_named_vector(&input_order))).size(14),
                    text(format!("y = [{}]^T", format_named_vector(&output_order))).size(14),
                    text(dimensions.clone()).size(13),
                ]
                .spacing(6),
            )
            .padding(12);

            content = content
                .push(
                    row![
                        text("Number of states").width(Length::Fixed(120.0)),
                        state_count_input,
                        text(dimensions.clone()).size(13),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .push(explanation)
                .push(self.matrix_editor_card_view(
                    MatrixKey::A,
                    build_state_labels(state_count),
                    build_state_labels(state_count),
                ))
                .push(self.matrix_editor_card_view(
                    MatrixKey::B,
                    build_state_labels(state_count),
                    input_order.clone(),
                ))
                .push(self.matrix_editor_card_view(
                    MatrixKey::C,
                    output_order.clone(),
                    build_state_labels(state_count),
                ))
                .push(self.matrix_editor_card_view(MatrixKey::D, output_order, input_order));
        }

        let save_button = if can_save {
            button("Save Model").on_press(Message::SaveModelPressed)
        } else {
            button("Save Model")
        };

        container(
            column![
                scrollable(content)
                    .direction(iced::widget::scrollable::Direction::Both {
                        vertical: iced::widget::scrollable::Scrollbar::new(),
                        horizontal: iced::widget::scrollable::Scrollbar::new(),
                    })
                    .height(Length::Fill),
                save_button
            ]
            .spacing(12),
        )
        .padding(12)
        .height(Length::Fill)
        .into()
    }

    fn simulation_editor_view(&self) -> Element<'_, Message> {
        let model_preview = build_model_preview(&self.model_editor, &self.variables);
        let simulation_running = self.simulation_running();
        let execution_active = self.execution_active();
        let structural_editable = !execution_active;
        let live_editable = simulation_running;
        let can_start = self.selected_project.is_some() && !execution_active;
        let can_stop = simulation_running;
        let can_save = self.selected_project.is_some() && !execution_active;

        let start_button = if can_start {
            button("Simulation Start").on_press(Message::StartSimulationPressed)
        } else {
            button("Simulation Start")
        };
        let stop_button = if can_stop {
            button("Simulation Stop").on_press(Message::StopSimulationPressed)
        } else {
            button("Simulation Stop")
        };
        let save_button = if can_save {
            button("Simulation speichern").on_press(Message::SaveSimulationPressed)
        } else {
            button("Simulation speichern")
        };

        let summary = match &self.active_snapshot {
            Some(ExecutionSnapshot::Simulation(snapshot)) => format!(
                "Simulation aktiv | Projekt={} | State={:?} | t={:.1}s | Cycles={} | LastDuration={}us",
                snapshot.project_name,
                snapshot.state,
                snapshot.simulation_time_s,
                snapshot.metrics.total_cycles,
                snapshot.metrics.last_duration_us
            ),
            Some(ExecutionSnapshot::Runtime(snapshot)) => format!(
                "Runtime aktiv | Projekt={} | State={:?}",
                snapshot.project_name, snapshot.state
            ),
            None => "Simulation nicht aktiv".to_string(),
        };

        let mut content = column![
            text("Simulate Controller").size(24),
            text("Offline closed-loop simulation from the stored continuous-time state-space model. No OPC-UA is used.").size(14),
            row![start_button, stop_button, save_button, text(summary).size(13)]
                .spacing(10)
                .align_y(iced::Alignment::Center)
        ]
        .spacing(12);

        if matches!(self.active_snapshot, Some(ExecutionSnapshot::Runtime(_))) {
            content =
                content.push(text("Runtime läuft: Für Simulation erst Runtime stoppen.").size(13));
        }

        if model_preview.is_none() {
            content = content.push(
                container(
                    column![
                        text("Zuerst ein lineares Zustandsraummodell definieren.").size(16),
                        text("Die Simulation benötigt das gespeicherte A/B/C/D-Modell aus \"Define Model\".")
                            .size(14),
                    ]
                    .spacing(8),
                )
                .padding(12),
            );
        } else {
            let speed_control: Element<'_, Message> = if structural_editable {
                pick_list(
                    SIMULATION_SPEED_OPTIONS,
                    Some(self.simulation_editor.speed),
                    Message::SimulationSpeedChanged,
                )
                .width(Length::Fixed(220.0))
                .into()
            } else {
                text(self.simulation_editor.speed.to_string())
                    .size(15)
                    .into()
            };

            content = content.push(
                row![
                    text("Simulation speed").width(Length::Fixed(140.0)),
                    speed_control,
                    text(if simulation_running {
                        "Live edits allowed: setpoints, disturbances, offsets, noise, A/B scales"
                    } else {
                        "Speed and initial conditions apply on next start"
                    })
                    .size(13),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );

            content = content
                .push(self.simulation_named_rows_section(
                    "Live setpoints",
                    "cv",
                    "soll",
                    &self.simulation_editor.cv_setpoints,
                    structural_editable || live_editable,
                    Message::SimulationCvSetpointChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "Initial CV values",
                    "cv",
                    "ist",
                    &self.simulation_editor.cv_initials,
                    structural_editable,
                    Message::SimulationCvInitialChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "Initial MV values",
                    "mv",
                    "read",
                    &self.simulation_editor.mv_initials,
                    structural_editable,
                    Message::SimulationMvInitialChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "Initial DV values",
                    "dv",
                    "read",
                    &self.simulation_editor.dv_initials,
                    structural_editable,
                    Message::SimulationDvInitialChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "DV disturbances",
                    "dv",
                    "read",
                    &self.simulation_editor.dv_disturbances,
                    structural_editable || live_editable,
                    Message::SimulationDvDisturbanceChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "CV offsets",
                    "cv",
                    "offset",
                    &self.simulation_editor.cv_offsets,
                    structural_editable || live_editable,
                    Message::SimulationCvOffsetChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "CV noise std",
                    "cv",
                    "noise_std",
                    &self.simulation_editor.cv_noise_std,
                    structural_editable || live_editable,
                    Message::SimulationCvNoiseChanged,
                ))
                .push(self.simulation_named_rows_section(
                    "MV noise std",
                    "mv",
                    "noise_std",
                    &self.simulation_editor.mv_noise_std,
                    structural_editable || live_editable,
                    Message::SimulationMvNoiseChanged,
                ))
                .push(
                    self.simulation_scale_matrix_view(
                        MatrixKey::A,
                        "A scale (%)",
                        &self.simulation_editor.a_scale_cells,
                        structural_editable || live_editable,
                        build_state_labels(
                            model_preview
                                .as_ref()
                                .and_then(ModelConfig::linear_state_space)
                                .map(|model| model.state_count)
                                .unwrap_or_default(),
                        ),
                        build_state_labels(
                            model_preview
                                .as_ref()
                                .and_then(ModelConfig::linear_state_space)
                                .map(|model| model.state_count)
                                .unwrap_or_default(),
                        ),
                    ),
                )
                .push(
                    self.simulation_scale_matrix_view(
                        MatrixKey::B,
                        "B scale (%)",
                        &self.simulation_editor.b_scale_cells,
                        structural_editable || live_editable,
                        build_state_labels(
                            model_preview
                                .as_ref()
                                .and_then(ModelConfig::linear_state_space)
                                .map(|model| model.state_count)
                                .unwrap_or_default(),
                        ),
                        model_preview
                            .as_ref()
                            .and_then(ModelConfig::linear_state_space)
                            .map(|model| model.u_order.clone())
                            .unwrap_or_default(),
                    ),
                );
        }

        container(
            scrollable(content)
                .direction(iced::widget::scrollable::Direction::Both {
                    vertical: iced::widget::scrollable::Scrollbar::new(),
                    horizontal: iced::widget::scrollable::Scrollbar::new(),
                })
                .height(Length::Fill),
        )
        .padding(12)
        .height(Length::Fill)
        .into()
    }

    fn simulation_named_rows_section<F>(
        &self,
        title: &str,
        prefix: &str,
        suffix: &str,
        rows: &[SimulationNamedValueRow],
        editable: bool,
        on_change: F,
    ) -> Element<'_, Message>
    where
        F: Fn(usize, String) -> Message + Copy + 'static,
    {
        let mut content = column![text(title.to_string()).size(18)].spacing(8);
        for (index, row_data) in rows.iter().enumerate() {
            let signal = format!("{}.{}.{}", prefix, row_data.name, suffix);
            let value_input = if editable {
                text_input("0.0", &row_data.value)
                    .on_input(move |value| on_change(index, value))
                    .width(Length::FillPortion(2))
            } else {
                text_input("0.0", &row_data.value).width(Length::FillPortion(2))
            };
            content = content.push(
                row![text(signal).width(Length::FillPortion(3)), value_input,]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
            );
        }

        container(content).padding(12).into()
    }

    fn simulation_scale_matrix_view(
        &self,
        matrix: MatrixKey,
        title: &str,
        cells: &[Vec<String>],
        editable: bool,
        row_labels: Vec<String>,
        col_labels: Vec<String>,
    ) -> Element<'_, Message> {
        let mut grid = column![].spacing(6);
        let mut header = row![text("").width(Length::Fixed(100.0))].spacing(6);
        for label in &col_labels {
            header = header.push(text(label.clone()).width(Length::Fixed(90.0)));
        }
        grid = grid.push(header);

        for (row_index, row_label) in row_labels.iter().enumerate() {
            let mut row_view = row![text(row_label.clone()).width(Length::Fixed(100.0))].spacing(6);
            if let Some(row_cells) = cells.get(row_index) {
                for (col_index, value) in row_cells.iter().enumerate() {
                    let input = if editable {
                        text_input("100.0", value)
                            .on_input(move |next| {
                                Message::SimulationScaleCellChanged(
                                    matrix, row_index, col_index, next,
                                )
                            })
                            .width(Length::Fixed(90.0))
                    } else {
                        text_input("100.0", value).width(Length::Fixed(90.0))
                    };
                    row_view = row_view.push(input);
                }
            }
            grid = grid.push(row_view);
        }

        container(column![text(title.to_string()).size(18), scrollable(grid)].spacing(10))
            .padding(12)
            .into()
    }

    fn matrix_editor_card_view(
        &self,
        matrix: MatrixKey,
        row_labels: Vec<String>,
        col_labels: Vec<String>,
    ) -> Element<'_, Message> {
        let cells = self.model_editor.cells(matrix);
        let mut grid = column![].spacing(6);
        let mut header = row![text("").width(Length::Fixed(100.0))].spacing(6);

        for label in &col_labels {
            header = header.push(text(label.clone()).width(Length::Fixed(90.0)));
        }

        grid = grid.push(header);

        if row_labels.is_empty() {
            grid = grid.push(text("No rows in this matrix.").size(13));
        } else {
            for (row_index, row_label) in row_labels.iter().enumerate() {
                let mut row_view =
                    row![text(row_label.clone()).width(Length::Fixed(100.0))].spacing(6);

                if let Some(row_cells) = cells.get(row_index) {
                    for (col_index, value) in row_cells.iter().enumerate() {
                        row_view = row_view.push(
                            text_input("0.0", value)
                                .on_input(move |next| {
                                    Message::ModelCellChanged(matrix, row_index, col_index, next)
                                })
                                .width(Length::Fixed(90.0)),
                        );
                    }
                }

                grid = grid.push(row_view);
            }
        }

        let import_hint = format!(
            "Paste {} as tab/newline data from Excel, or use comma/semicolon separated rows.",
            matrix.label()
        );
        let import_button = if !row_labels.is_empty() || !col_labels.is_empty() {
            button("Apply import").on_press(Message::ApplyModelImportPressed(matrix))
        } else {
            button("Apply import")
        };

        container(
            column![
                text(format!(
                    "{} matrix ({} x {})",
                    matrix.label(),
                    row_labels.len(),
                    col_labels.len()
                ))
                .size(18),
                scrollable(grid)
                    .direction(iced::widget::scrollable::Direction::Both {
                        vertical: iced::widget::scrollable::Scrollbar::new(),
                        horizontal: iced::widget::scrollable::Scrollbar::new(),
                    })
                    .height(Length::Shrink),
                text(import_hint).size(13),
                text_editor(self.model_editor.import_content(matrix))
                    .height(Length::Fixed(100.0))
                    .on_action(move |action| Message::ModelImportEdited(matrix, action)),
                import_button,
            ]
            .spacing(10),
        )
        .padding(12)
        .into()
    }

    fn control_function_tests_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_run = self.selected_project.is_some() && !runtime_running;
        let run_button = if can_run {
            button("Tests ausführen").on_press(Message::RunControlFunctionTestsPressed)
        } else {
            button("Tests ausführen")
        };

        let runtime_hint = if runtime_running {
            "Ausführung läuft: Offline-Tests sind erst nach Stop verfügbar."
        } else {
            "Die aktive Regellogik wird offline geprüft. Bei großen Räumen wird automatisch auf Einzel-Sweeps pro Dimension zurückgefallen."
        };

        let mut content = column![
            text("Control Function Tests").size(24),
            text("Sweep-Tests prüfen die Funktion über Eingänge und Online-Parameter, erfassen Fehlerfälle und messen Ausführungszeiten.").size(15),
            row![run_button, text(runtime_hint).size(13)]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            text(self.control_function_case_estimate_label()).size(13),
        ]
        .spacing(10);

        content = content.push(text("Input Sweeps").size(18));
        if self.control_function_tests.input_rows.is_empty() {
            content = content.push(text("Keine Eingänge konfiguriert."));
        } else {
            for (index, row_data) in self.control_function_tests.input_rows.iter().enumerate() {
                content = content.push(self.control_function_sweep_row_view(
                    index,
                    row_data,
                    Message::ControlTestInputEnabledToggled,
                    Message::ControlTestInputMinChanged,
                    Message::ControlTestInputMaxChanged,
                    Message::ControlTestInputSamplesChanged,
                ));
            }
        }

        content = content.push(text("Online Parameter Sweeps").size(18));
        if self.control_function_tests.param_rows.is_empty() {
            content = content.push(text("Keine Online-Parameter aktiv."));
        } else {
            for (index, row_data) in self.control_function_tests.param_rows.iter().enumerate() {
                content = content.push(self.control_function_sweep_row_view(
                    index,
                    row_data,
                    Message::ControlTestParamEnabledToggled,
                    Message::ControlTestParamMinChanged,
                    Message::ControlTestParamMaxChanged,
                    Message::ControlTestParamSamplesChanged,
                ));
            }
        }

        if let Some(report) = &self.control_function_tests.last_report {
            let summary = container(
                column![
                    row![
                        text("Modus").width(Length::Fixed(160.0)),
                        text(format_sweep_mode(report.execution.mode)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("Fälle").width(Length::Fixed(160.0)),
                        text(report.execution.executed_case_count.to_string()),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("Suite gesamt").width(Length::Fixed(160.0)),
                        text(format!("{:.1} ms", report.execution.total_wall_time_ms)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    text("Runtime").size(15),
                    row![
                        text("min").width(Length::Fixed(160.0)),
                        text(format!("{} us", report.execution.duration_min_us)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("avg").width(Length::Fixed(160.0)),
                        text(format!("{:.1} us", report.execution.duration_avg_us)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("p95").width(Length::Fixed(160.0)),
                        text(format!("{} us", report.execution.duration_p95_us)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("max").width(Length::Fixed(160.0)),
                        text(format!("{} us", report.execution.duration_max_us)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    text("Validierung").size(15),
                    row![
                        text("Status").width(Length::Fixed(160.0)),
                        text(format!(
                            "{} ok | {} warnings | {} errors",
                            report.counts.ok_cases,
                            report.counts.warning_cases,
                            report.counts.error_cases
                        )),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    row![
                        text("Output-Prüfung").width(Length::Fixed(160.0)),
                        text(format!(
                            "{} invalid | {} außerhalb Limits | {} unerwartet",
                            report.outputs.invalid_output_cases,
                            report.outputs.out_of_limit_cases,
                            report.outputs.unexpected_output_cases
                        )),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                ]
                .spacing(6),
            )
            .padding(10);

            content = content.push(text("Letzter Lauf").size(18)).push(summary);

            if !report.issue_cases.is_empty() {
                content = content.push(text("Auffällige Fälle").size(16));
                for case in &report.issue_cases {
                    content = content.push(text(format_case_preview(case)).size(14));
                    for detail in &case.details {
                        content = content.push(text(format!("  - {}", detail)).size(13));
                    }
                }
            }

            if !report.slowest_cases.is_empty() {
                content = content.push(text("Langsamste Fälle").size(16));
                for case in &report.slowest_cases {
                    content = content.push(text(format_case_preview(case)).size(14));
                }
            }
        }

        container(scrollable(content).height(Length::Fill))
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn control_function_sweep_row_view<FEnabled, FMin, FMax, FSamples>(
        &self,
        index: usize,
        row_data: &ControlFunctionSweepRowState,
        enabled_message: FEnabled,
        min_message: FMin,
        max_message: FMax,
        samples_message: FSamples,
    ) -> Element<'_, Message>
    where
        FEnabled: Fn(usize, bool) -> Message + Copy + 'static,
        FMin: Fn(usize, String) -> Message + Copy + 'static,
        FMax: Fn(usize, String) -> Message + Copy + 'static,
        FSamples: Fn(usize, String) -> Message + Copy + 'static,
    {
        let toggle = checkbox(row_data.enabled)
            .label(row_data.name.clone())
            .on_toggle(move |checked| enabled_message(index, checked))
            .width(Length::FillPortion(3));
        let baseline = text(format!("Basis: {}", row_data.baseline_label))
            .width(Length::FillPortion(2))
            .size(13);

        let row = match row_data.value_kind {
            SweepValueKind::Bool => row![
                toggle,
                baseline,
                text("Sweep: false/true")
                    .width(Length::FillPortion(2))
                    .size(13),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
            SweepValueKind::F64 => row![
                toggle,
                baseline,
                text_input("min", &row_data.min)
                    .on_input(move |value| min_message(index, value))
                    .width(Length::FillPortion(1)),
                text_input("max", &row_data.max)
                    .on_input(move |value| max_message(index, value))
                    .width(Length::FillPortion(1)),
                text_input("samples", &row_data.samples)
                    .on_input(move |value| samples_message(index, value))
                    .width(Length::FillPortion(1)),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        };

        container(row).padding(6).into()
    }

    fn communication_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_save = self.selected_project.is_some() && !runtime_running;

        let save_button = if can_save {
            button("Kommunikation speichern").on_press(Message::SaveCommunicationPressed)
        } else {
            button("Kommunikation speichern")
        };

        let runtime_hint = if runtime_running {
            "Ausführung läuft: Für Kommunikations-Änderungen erst stoppen."
        } else {
            ""
        };

        container(
            column![
                text("OPC-UA Basis-Konfiguration").size(20),
                row![
                    text("Protokoll").width(Length::Fixed(120.0)),
                    text("opc_ua").size(15),
                    text("(V1 fest)").size(13)
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                row![
                    text("Server Adresse").width(Length::Fixed(120.0)),
                    text_input(
                        "opc.tcp://127.0.0.1:4840",
                        &self.communication_editor.endpoint_url
                    )
                    .on_input(Message::CommunicationEndpointChanged)
                    .width(Length::Fill)
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                row![
                    text("Kommunikationszeit")
                        .width(Length::Fixed(120.0)),
                    text_input("io_cycle_ms", &self.communication_editor.io_cycle_ms)
                        .on_input(Message::CommunicationCycleChanged)
                        .width(Length::Fixed(180.0)),
                    text("ms: Read/Write Intervall für OPC-UA I/O").size(14),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                text("Hinweis: Effektive minimale I/O-Rate wird zusätzlich durch runtime.cycle_ms begrenzt.")
                    .size(13),
                text(runtime_hint).size(13),
                save_button
            ]
            .spacing(10),
        )
        .padding(10)
        .into()
    }

    fn watchdog_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_save = self.selected_project.is_some() && !runtime_running;

        let kind_pick = pick_list(
            WATCHDOG_KIND_OPTIONS.to_vec(),
            Some(self.watchdog_editor.kind),
            Message::WatchdogKindChanged,
        )
        .width(Length::Fixed(200.0));

        let mut content = column![
            text("Watchdog Editor").size(20),
            row![
                text("Typ").width(Length::Fixed(70.0)),
                kind_pick,
                text(if runtime_running {
                    "Ausführung läuft: Speichern deaktiviert"
                } else {
                    ""
                })
                .size(13)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(8);

        if self.watchdog_editor.kind == WatchdogKindChoice::CounterMirror {
            content = content
                .push(
                    row![
                        text("Read Tag").width(Length::Fixed(90.0)),
                        text_input(
                            "OPC-UA Tagname (z.B. ns=2;s=WD.Counter)",
                            &self.watchdog_editor.read_tag
                        )
                        .on_input(Message::WatchdogReadTagChanged)
                        .width(Length::Fill)
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .push(
                    row![
                        text("Write Tag").width(Length::Fixed(90.0)),
                        text_input(
                            "OPC-UA Tagname (z.B. ns=2;s=WD.Mirror)",
                            &self.watchdog_editor.write_tag
                        )
                        .on_input(Message::WatchdogWriteTagChanged)
                        .width(Length::Fill)
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .push(
                    row![
                        text("Zyklus").width(Length::Fixed(90.0)),
                        text_input("mirror_cycle_ms", &self.watchdog_editor.mirror_cycle_ms)
                            .on_input(Message::WatchdogCycleChanged)
                            .width(Length::Fixed(180.0)),
                        text("ms: Read -> Write Intervall").size(14)
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .push(
                    row![
                        text("Timeout").width(Length::Fixed(90.0)),
                        text_input("stale_timeout_ms", &self.watchdog_editor.stale_timeout_ms)
                            .on_input(Message::WatchdogTimeoutChanged)
                            .width(Length::Fixed(180.0)),
                        text("ms: Fault wenn Read nicht mehr ändert").size(14)
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .push(text("Hinweis: Read/Write sind OPC-UA Tagnamen der Variablen.").size(13));
        }

        let save_button = if can_save {
            button("Watchdog speichern").on_press(Message::SaveWatchdogPressed)
        } else {
            button("Watchdog speichern")
        };
        let test_button = if can_save {
            button("Watchdog testen").on_press(Message::TestWatchdogPressed)
        } else {
            button("Watchdog testen")
        };

        container(content.push(row![save_button, test_button].spacing(10)))
            .padding(10)
            .into()
    }

    fn io_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_save = self.selected_project.is_some() && !runtime_running;
        let mut mv_col = column![
            row![
                text("MVs (manipulierende Variablen)").size(18),
                button("+ Zeile").on_press(Message::AddMvNameRow)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(6);

        for (index, row_data) in self.mv_name_rows.iter().enumerate() {
            mv_col = mv_col.push(
                text_input("MV Name", &row_data.name)
                    .on_input(move |value| Message::MvNameChanged(index, value))
                    .width(Length::Fill),
            );
        }

        let mut cv_col = column![
            row![
                text("CVs (Regelgrößen)").size(18),
                button("+ Zeile").on_press(Message::AddCvNameRow)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(6);

        for (index, row_data) in self.cv_name_rows.iter().enumerate() {
            cv_col = cv_col.push(
                text_input("CV Name", &row_data.name)
                    .on_input(move |value| Message::CvNameChanged(index, value))
                    .width(Length::Fill),
            );
        }

        let mut dv_col = column![
            row![
                text("DVs (Störgrößen)").size(18),
                button("+ Zeile").on_press(Message::AddDvNameRow)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(6);

        for (index, row_data) in self.dv_name_rows.iter().enumerate() {
            dv_col = dv_col.push(
                text_input("DV Name", &row_data.name)
                    .on_input(move |value| Message::DvNameChanged(index, value))
                    .width(Length::Fill),
            );
        }

        let save_button = if can_save {
            button("Variablen speichern").on_press(Message::SaveVariablesPressed)
        } else {
            button("Variablen speichern")
        };

        let hint = if runtime_running {
            text("Ausführung läuft: Für strukturelle Änderungen erst stoppen.").size(13)
        } else {
            text("").size(13)
        };

        let main_content = column![
            text("Ein- und Ausgänge definieren").size(20),
            text("Hier werden nur Namen für MV/CV/DV festgelegt. OPC-UA-Adressen folgen im nächsten Schritt \"I/O definieren\".").size(13),
            mv_col,
            cv_col,
            dv_col,
        ]
        .spacing(10);

        container(
            column![
                scrollable(main_content)
                    .direction(iced::widget::scrollable::Direction::Both {
                        vertical: iced::widget::scrollable::Scrollbar::new(),
                        horizontal: iced::widget::scrollable::Scrollbar::new(),
                    })
                    .height(Length::Fill),
                hint,
                save_button
            ]
            .spacing(10),
        )
        .padding(10)
        .height(Length::Fill)
        .into()
    }

    fn limits_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_save = self.selected_project.is_some() && !runtime_running;
        let can_test = self.selected_project.is_some();
        let has_any_names = self.variables.has_any();
        let mut content = column![text("I/O definieren").size(20)].spacing(10);

        if !has_any_names {
            content = content.push(text(
                "Keine MV/CV/DV-Namen vorhanden. Bitte zuerst \"Ein- und Ausgänge definieren\" ausfüllen.",
            ));
        } else {
            let header = row![
                text("Unterparameter").width(Length::FillPortion(2)),
                text("opc address").width(Length::FillPortion(3)),
                text("optional init value").width(Length::FillPortion(2)),
                text("read value").width(Length::FillPortion(2)),
                text("write value").width(Length::FillPortion(2)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);

            content = content.push(header);
            content = content.push(
                text("Hinweis: test_read/test_write laufen über die Buttons unten; test_write nutzt befüllte write-value-Felder.")
                    .size(13),
            );
            content = content.push(
                text("MV: MV_WRITE/MV_READ/HL/LL/Enable mit optionaler OPC-Adresse (für Abschluss nicht Pflicht)")
                    .size(13),
            );
            for (index, row_data) in self.mv_io_rows.iter().enumerate() {
                content = content.push(
                    column![
                        text(format!("MV: {}", row_data.name)).size(16),
                        row![
                            text("MV_WRITE").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.mv_write)
                                .on_input(move |value| Message::MvWriteChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_mv_write)
                                .width(Length::FillPortion(2)),
                            text_input("write", &row_data.test_write)
                                .on_input(move |value| Message::MvTestWriteChanged(index, value))
                                .width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("MV_READ").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.mv_read)
                                .on_input(move |value| Message::MvReadChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_mv_read)
                                .width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("HL").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.hl_opc)
                                .on_input(move |value| Message::MvHlOpcChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", &row_data.hl)
                                .on_input(move |value| Message::MvHlChanged(index, value))
                                .width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_hl).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("LL").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.ll_opc)
                                .on_input(move |value| Message::MvLlOpcChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", &row_data.ll)
                                .on_input(move |value| Message::MvLlChanged(index, value))
                                .width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_ll).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("Enable").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.enable)
                                .on_input(move |value| Message::MvEnableChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_enable).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(8),
                );
            }

            content = content.push(
                text("CV: SOLL/IST/Sollband/Enable mit optionaler OPC-Adresse (Sollband alternativ als init value)")
                    .size(13),
            );
            for (index, row_data) in self.cv_io_rows.iter().enumerate() {
                content = content.push(
                    column![
                        text(format!("CV: {}", row_data.name)).size(16),
                        row![
                            text("SOLL").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.soll)
                                .on_input(move |value| Message::CvSollChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_soll).width(Length::FillPortion(2)),
                            text_input("write", &row_data.test_write)
                                .on_input(move |value| Message::CvTestWriteChanged(index, value))
                                .width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("IST").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.ist)
                                .on_input(move |value| Message::CvIstChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_ist).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("Sollband").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.sollband_opc)
                                .on_input(move |value| Message::CvSollbandOpcChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", &row_data.sollband_value)
                                .on_input(move |value| Message::CvSollbandValueChanged(
                                    index, value
                                ))
                                .width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_sollband)
                                .width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("Enable").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.enable)
                                .on_input(move |value| Message::CvEnableChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_enable).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(8),
                );
            }

            content = content.push(
                text("DV: READ/Enable mit optionaler OPC-Adresse (für Abschluss nicht Pflicht)")
                    .size(13),
            );
            for (index, row_data) in self.dv_io_rows.iter().enumerate() {
                content = content.push(
                    column![
                        text(format!("DV: {}", row_data.name)).size(16),
                        row![
                            text("READ").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.read)
                                .on_input(move |value| Message::DvReadChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_read).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("Enable").width(Length::FillPortion(2)),
                            text_input("opc", &row_data.enable)
                                .on_input(move |value| Message::DvEnableChanged(index, value))
                                .width(Length::FillPortion(3)),
                            text_input("init", "").width(Length::FillPortion(2)),
                            text_input("read", &row_data.read_enable).width(Length::FillPortion(2)),
                            text_input("write", "").width(Length::FillPortion(2)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(8),
                );
            }
        }

        let save_button = if can_save {
            button("I/O Definition speichern").on_press(Message::SaveIoDefinitionPressed)
        } else {
            button("I/O Definition speichern")
        };
        let test_read_button = if can_test {
            button("test_read").on_press(Message::TestIoDefinitionReadPressed)
        } else {
            button("test_read")
        };
        let test_write_button = if can_test {
            button("test_write").on_press(Message::TestIoDefinitionWritePressed)
        } else {
            button("test_write")
        };

        let hint = if runtime_running {
            text("Ausführung läuft: Für strukturelle Änderungen erst stoppen.").size(13)
        } else {
            text("").size(13)
        };

        let actions = row![save_button, test_read_button, test_write_button].spacing(10);

        container(
            column![
                scrollable(content)
                    .direction(iced::widget::scrollable::Direction::Both {
                        vertical: iced::widget::scrollable::Scrollbar::new(),
                        horizontal: iced::widget::scrollable::Scrollbar::new(),
                    })
                    .height(Length::Fill),
                hint,
                actions
            ]
            .spacing(10),
        )
        .padding(10)
        .height(Length::Fill)
        .into()
    }

    fn logic_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_save = self.selected_project.is_some() && !runtime_running;
        let can_run_custom_actions = self.selected_project.is_some() && !runtime_running;

        let mut available = vec![LogicKindChoice::PController];
        if self.logic_editor.custom_compatible {
            available.push(LogicKindChoice::CustomRust);
        }
        let selected_kind = if available.contains(&self.logic_editor.selected_kind) {
            self.logic_editor.selected_kind
        } else {
            LogicKindChoice::PController
        };

        let kind_pick = pick_list(
            available.clone(),
            Some(selected_kind),
            Message::LogicKindChanged,
        )
        .width(Length::Fixed(260.0));

        let mut content = column![
            text("Regelstrategie").size(20),
            row![text("Auswahl").width(Length::Fixed(90.0)), kind_pick]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            text("Nur kompatible Strategien für die aktuelle I/O-Definition erscheinen in der Auswahl.")
                .size(13),
        ]
        .spacing(10);

        if selected_kind == LogicKindChoice::PController {
            content = content.push(
                row![
                    text("P-Regler CV->MV Zuordnung").size(16),
                    button("+ Paar").on_press(Message::AddPControllerPair)
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
            );

            let header = row![
                text("CV").width(Length::FillPortion(2)),
                text("MV").width(Length::FillPortion(2)),
                text("kp").width(Length::FillPortion(1)),
                text("bias").width(Length::FillPortion(1)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            content = content.push(header);

            for (index, row_data) in self.logic_editor.p_pairs.iter().enumerate() {
                content = content.push(
                    row![
                        text_input("cv name", &row_data.cv)
                            .on_input(move |value| Message::PPairCvChanged(index, value))
                            .width(Length::FillPortion(2)),
                        text_input("mv name", &row_data.mv)
                            .on_input(move |value| Message::PPairMvChanged(index, value))
                            .width(Length::FillPortion(2)),
                        text_input("kp", &row_data.kp)
                            .on_input(move |value| Message::PPairKpChanged(index, value))
                            .width(Length::FillPortion(1)),
                        text_input("bias", &row_data.bias)
                            .on_input(move |value| Message::PPairBiasChanged(index, value))
                            .width(Length::FillPortion(1)),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );
            }
        } else {
            content = content.push(text("Eigene Regellogik ist aktiv.").size(14));
        }

        let template_button = if can_run_custom_actions {
            button("Template generieren").on_press(Message::GenerateLogicTemplatePressed)
        } else {
            button("Template generieren")
        };
        let build_button = if can_run_custom_actions {
            button("Strategie prüfen/builden").on_press(Message::BuildCustomLogicPressed)
        } else {
            button("Strategie prüfen/builden")
        };

        content = content.push(text("Custom Rust (cdylib)").size(16)).push(
            column![
                row![
                    text("Name").width(Length::Fixed(90.0)),
                    text_input("strategy name", &self.logic_editor.custom_strategy_name)
                        .on_input(Message::CustomStrategyNameChanged)
                        .width(Length::Fill)
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                row![
                    text("Source").width(Length::Fixed(90.0)),
                    text_input(
                        "logic/custom_strategy/src/lib.rs",
                        &self.logic_editor.custom_source_path
                    )
                    .on_input(Message::CustomSourcePathChanged)
                    .width(Length::Fill)
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                row![
                    text("Artifact").width(Length::Fixed(90.0)),
                    text_input(
                        "logic/custom_strategy/target/release/<name>.dll",
                        &self.logic_editor.custom_artifact_path
                    )
                    .on_input(Message::CustomArtifactPathChanged)
                    .width(Length::Fill)
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                row![template_button, build_button].spacing(10),
                text(format!(
                    "Status: {}{}",
                    if self.logic_editor.custom_compatible {
                        "kompatibel"
                    } else {
                        "nicht kompatibel"
                    },
                    if self.logic_editor.custom_status.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" | {}", self.logic_editor.custom_status)
                    }
                ))
                .size(13),
            ]
            .spacing(8),
        );

        if !self.logic_editor.custom_compatible {
            content = content.push(text(
                "Hinweis: 'Eigene Regellogik' wird erst nach erfolgreichem Build+Kompatibilitätscheck auswählbar.",
            ));
        }

        let save_button = if can_save {
            button("Regellogik speichern").on_press(Message::SaveLogicPressed)
        } else {
            button("Regellogik speichern")
        };

        container(column![scrollable(content).height(Length::Fill), save_button].spacing(10))
            .padding(10)
            .height(Length::Fill)
            .into()
    }

    fn params_view(&self) -> Element<'_, Message> {
        let execution_active = self.execution_active();
        let mut content = column![text("Online Parameter").size(20)].spacing(10);

        if self.has_configured_variables() {
            content =
                content.push(text("Konfigurierte Variablen (für Regellogik verfügbar):").size(15));
            for line in self.configured_variable_lines() {
                content = content.push(text(line));
            }
            content = content.push(text(" "));
        }

        if self.logic_params.is_empty() {
            content = content.push(text("Keine Parameter vorhanden."));
        } else {
            let mut keys: Vec<_> = self.logic_params.keys().cloned().collect();
            keys.sort();

            for key in keys {
                let current = self
                    .logic_params
                    .get(&key)
                    .cloned()
                    .unwrap_or(LogicParamValue::F64(0.0));
                let input_value = self
                    .param_inputs
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| format_logic_param_value(&current));
                let key_clone = key.clone();

                let apply_button = if execution_active {
                    button("Apply").on_press(Message::ApplyParamPressed(key_clone))
                } else {
                    button("Apply")
                };

                content = content.push(
                    row![
                        text(key.clone()).width(Length::FillPortion(2)),
                        text(format_logic_param_value(&current)).width(Length::FillPortion(1)),
                        text_input("Neuer Wert", &input_value)
                            .on_input(move |value| Message::ParamInputChanged(key.clone(), value))
                            .width(Length::FillPortion(2)),
                        apply_button,
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                );
            }
        }

        container(scrollable(content).height(Length::Fixed(220.0)))
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    fn plots_view(&self) -> Element<'_, Message> {
        let open_web_button = if self.web_bind.is_some() {
            button("Interaktive Web-Plots öffnen").on_press(Message::OpenPlotsWebPressed)
        } else {
            button("Interaktive Web-Plots öffnen")
        };

        let web_hint = if self.web_bind.is_some() {
            "Öffnet /plots der lokalen Web-API im Browser."
        } else {
            "Web API deaktiviert. Für interaktive Plots Desktop ohne --no-web starten."
        };

        let mut content = column![
            text("Plots (SQLite)").size(20),
            row![open_web_button, text(web_hint).size(13)]
                .spacing(10)
                .align_y(iced::Alignment::Center),
        ]
        .spacing(8);
        let plot_lines = build_plot_lines(&self.samples, 48);

        if plot_lines.is_empty() {
            content = content.push(text("Noch keine Plotdaten vorhanden."));
        } else {
            for line in plot_lines {
                content = content.push(text(line).size(14));
            }
        }

        container(scrollable(content).height(Length::Fill))
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn plots_web_url(&self) -> Option<String> {
        self.web_bind.map(|bind| {
            if let Some(project_name) = &self.selected_project {
                format!("http://{bind}/plots?project={project_name}")
            } else {
                format!("http://{bind}/plots")
            }
        })
    }

    fn control_function_case_estimate_label(&self) -> String {
        match build_control_function_sweep_selections(&self.control_function_tests) {
            Ok(selections) => {
                let mut full_factorial_cases = 1usize;
                let mut single_axis_cases = 1usize;
                let mut active_dimensions = 0usize;

                for selection in &selections {
                    if !selection.enabled {
                        continue;
                    }
                    active_dimensions += 1;
                    let count = match &selection.numeric {
                        Some(numeric) => numeric.samples.max(1),
                        None => 2,
                    };
                    full_factorial_cases = full_factorial_cases.saturating_mul(count);
                    single_axis_cases = single_axis_cases.saturating_add(count);
                }

                if active_dimensions == 0 {
                    "Fallraum: nur Basisfall".to_string()
                } else if full_factorial_cases <= 4_096 {
                    format!(
                        "Fallraum: Vollfaktoriell {} + 1 Basisfall",
                        full_factorial_cases
                    )
                } else {
                    format!(
                        "Fallraum: Vollfaktoriell {} zu groß, Fallback auf {} Einzel-Sweeps + Basisfall",
                        full_factorial_cases,
                        single_axis_cases - 1
                    )
                }
            }
            Err(err) => format!("Sweep-Konfiguration prüfen: {err}"),
        }
    }

    fn status_view(&self) -> Element<'_, Message> {
        let mut line = self.status_message.clone();
        if let Some(err) = &self.error_message {
            line = format!("{} | Fehler: {}", line, err);
        }
        container(text(line).size(14)).padding(8).into()
    }

    fn refresh_param_inputs_full(&mut self) {
        self.param_dirty_keys.clear();
        let mut next = HashMap::new();
        for (key, value) in &self.logic_params {
            next.insert(key.clone(), format_logic_param_value(value));
        }
        self.param_inputs = next;
    }

    fn refresh_param_inputs_preserving_dirty(&mut self) {
        self.param_inputs
            .retain(|key, _| self.logic_params.contains_key(key));
        self.param_dirty_keys
            .retain(|key| self.logic_params.contains_key(key));

        for (key, value) in &self.logic_params {
            if !self.param_dirty_keys.contains(key) {
                self.param_inputs
                    .insert(key.clone(), format_logic_param_value(value));
            } else if !self.param_inputs.contains_key(key) {
                self.param_inputs
                    .insert(key.clone(), format_logic_param_value(value));
            }
        }
    }

    fn sync_io_rows_from_name_rows(&mut self) {
        let mvs = collect_variable_names(&self.mv_name_rows);
        let cvs = collect_variable_names(&self.cv_name_rows);
        let dvs = collect_variable_names(&self.dv_name_rows);

        self.variables = VariableCatalogConfig {
            mvs: mvs.clone(),
            cvs: cvs.clone(),
            dvs: dvs.clone(),
        };
        self.model_editor.resize_matrices_for(&self.variables);
        self.simulation_editor.resize_for(
            &self.variables,
            build_model_preview(&self.model_editor, &self.variables).as_ref(),
        );
        self.mv_io_rows = sync_mv_rows(&mvs, &self.mv_io_rows);
        self.cv_io_rows = sync_cv_rows(&cvs, &self.cv_io_rows);
        self.dv_io_rows = sync_dv_rows(&dvs, &self.dv_io_rows);

        if self.mv_name_rows.is_empty() {
            self.mv_name_rows.push(VariableNameRow::blank());
        }
        if self.cv_name_rows.is_empty() {
            self.cv_name_rows.push(VariableNameRow::blank());
        }
        if self.dv_name_rows.is_empty() {
            self.dv_name_rows.push(VariableNameRow::blank());
        }
    }

    fn configured_variable_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.variables.mvs.is_empty() {
            lines.push(format!("MV: {}", self.variables.mvs.join(", ")));
        }
        if !self.variables.cvs.is_empty() {
            lines.push(format!("CV: {}", self.variables.cvs.join(", ")));
        }
        if !self.variables.dvs.is_empty() {
            lines.push(format!("DV: {}", self.variables.dvs.join(", ")));
        }
        lines
    }

    fn has_configured_variables(&self) -> bool {
        self.variables.has_any()
            || !collect_variable_names(&self.mv_name_rows).is_empty()
            || !collect_variable_names(&self.cv_name_rows).is_empty()
            || !collect_variable_names(&self.dv_name_rows).is_empty()
    }
}

fn format_logic_param_value(value: &LogicParamValue) -> String {
    match value {
        LogicParamValue::F64(v) => format!("{v:.4}"),
        LogicParamValue::Bool(v) => v.to_string(),
    }
}

fn parse_logic_param_input(
    current: &LogicParamValue,
    raw: &str,
) -> Result<LogicParamValue, String> {
    match current {
        LogicParamValue::F64(_) => raw
            .trim()
            .parse::<f64>()
            .map(LogicParamValue::F64)
            .map_err(|_| "erwarte Zahl (f64)".to_string()),
        LogicParamValue::Bool(_) => parse_bool(raw)
            .map(LogicParamValue::Bool)
            .ok_or_else(|| "erwarte bool: true/false/1/0".to_string()),
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn collect_variable_names(rows: &[VariableNameRow]) -> Vec<String> {
    let mut out = Vec::new();
    for row in rows {
        let name = row.name.trim();
        if name.is_empty() {
            continue;
        }
        if !out.iter().any(|existing: &String| existing == name) {
            out.push(name.to_string());
        }
    }
    out
}

fn sync_mv_rows(names: &[String], existing: &[MvIoEditorRow]) -> Vec<MvIoEditorRow> {
    let mut out = Vec::new();
    for name in names {
        if let Some(row) = existing.iter().find(|row| row.name == *name) {
            out.push(row.clone());
        } else {
            out.push(MvIoEditorRow {
                name: name.clone(),
                ..MvIoEditorRow::blank()
            });
        }
    }
    out
}

fn sync_cv_rows(names: &[String], existing: &[CvIoEditorRow]) -> Vec<CvIoEditorRow> {
    let mut out = Vec::new();
    for name in names {
        if let Some(row) = existing.iter().find(|row| row.name == *name) {
            out.push(row.clone());
        } else {
            out.push(CvIoEditorRow {
                name: name.clone(),
                ..CvIoEditorRow::blank()
            });
        }
    }
    out
}

fn sync_dv_rows(names: &[String], existing: &[DvIoEditorRow]) -> Vec<DvIoEditorRow> {
    let mut out = Vec::new();
    for name in names {
        if let Some(row) = existing.iter().find(|row| row.name == *name) {
            out.push(row.clone());
        } else {
            out.push(DvIoEditorRow {
                name: name.clone(),
                ..DvIoEditorRow::blank()
            });
        }
    }
    out
}

fn load_projects_task(project_service: ProjectService) -> Task<Message> {
    Task::perform(
        async move {
            project_service
                .list_projects()
                .map_err(|err| format!("Projekte konnten nicht geladen werden: {err}"))
        },
        Message::ProjectsLoaded,
    )
}

fn load_project_panel_task(project_service: ProjectService, project_name: String) -> Task<Message> {
    Task::perform(
        async move {
            let sidebar_tasks = project_service
                .sidebar_tasks(&project_name)
                .map_err(|err| format!("Sidebar-Aufgaben konnten nicht geladen werden: {err}"))?;
            let cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            let communication = CommunicationEditorState::from_config(&cfg.communication);
            let model_editor = ModelEditorState::from_config(cfg.model.as_ref(), &cfg.variables);
            let simulation_editor = SimulationEditorState::from_config(
                &cfg.simulation,
                &cfg.variables,
                cfg.model.as_ref(),
            );
            let logic_editor = LogicEditorState::from_config(&cfg.logic, &cfg.io);
            let control_function_tests =
                ControlFunctionTestState::from_dimensions(&default_sweep_dimensions(&cfg));

            let mv_names = if cfg.variables.mvs.is_empty() {
                vec![VariableNameRow::blank()]
            } else {
                cfg.variables
                    .mvs
                    .iter()
                    .map(|name| VariableNameRow { name: name.clone() })
                    .collect()
            };
            let cv_names = if cfg.variables.cvs.is_empty() {
                vec![VariableNameRow::blank()]
            } else {
                cfg.variables
                    .cvs
                    .iter()
                    .map(|name| VariableNameRow { name: name.clone() })
                    .collect()
            };
            let dv_names = if cfg.variables.dvs.is_empty() {
                vec![VariableNameRow::blank()]
            } else {
                cfg.variables
                    .dvs
                    .iter()
                    .map(|name| VariableNameRow { name: name.clone() })
                    .collect()
            };

            let mv_io_existing: Vec<_> = cfg
                .io_definition
                .mvs
                .iter()
                .map(MvIoEditorRow::from_definition)
                .collect();
            let cv_io_existing: Vec<_> = cfg
                .io_definition
                .cvs
                .iter()
                .map(CvIoEditorRow::from_definition)
                .collect();
            let dv_io_existing: Vec<_> = cfg
                .io_definition
                .dvs
                .iter()
                .map(DvIoEditorRow::from_definition)
                .collect();

            let mv_io_rows = sync_mv_rows(&cfg.variables.mvs, &mv_io_existing);
            let cv_io_rows = sync_cv_rows(&cfg.variables.cvs, &cv_io_existing);
            let dv_io_rows = sync_dv_rows(&cfg.variables.dvs, &dv_io_existing);

            Ok(ProjectPanelData {
                sidebar_tasks,
                config_params: cfg.logic.params.clone(),
                logic_editor,
                model_editor,
                simulation_editor,
                control_function_tests,
                communication,
                variables: cfg.variables.clone(),
                mv_names,
                cv_names,
                dv_names,
                mv_io_rows,
                cv_io_rows,
                dv_io_rows,
                watchdog: WatchdogEditorState::from_config(&cfg.watchdog),
            })
        },
        Message::ProjectPanelLoaded,
    )
}

fn create_project_task(project_service: ProjectService, project_name: String) -> Task<Message> {
    Task::perform(
        async move {
            project_service
                .create_project(&project_name)
                .map(|_| project_name)
                .map_err(|err| format!("Projekt konnte nicht erstellt werden: {err}"))
        },
        Message::ProjectCreated,
    )
}

fn start_runtime_task(
    runtime: Arc<RuntimeManager>,
    project_service: ProjectService,
    project_name: String,
) -> Task<Message> {
    Task::perform(
        async move {
            runtime
                .start(&project_service, &project_name)
                .await
                .map_err(|err| format!("Runtime konnte nicht gestartet werden: {err}"))?;
            Ok(format!("Runtime für '{}' gestartet", project_name))
        },
        Message::RuntimeActionFinished,
    )
}

fn stop_runtime_task(runtime: Arc<RuntimeManager>) -> Task<Message> {
    Task::perform(
        async move {
            runtime
                .stop()
                .await
                .map_err(|err| format!("Runtime konnte nicht gestoppt werden: {err}"))?;
            Ok("Runtime gestoppt".to_string())
        },
        Message::RuntimeActionFinished,
    )
}

fn start_simulation_task(
    runtime: Arc<RuntimeManager>,
    project_service: ProjectService,
    project_name: String,
) -> Task<Message> {
    Task::perform(
        async move {
            runtime
                .start_simulation(&project_service, &project_name)
                .await
                .map_err(|err| format!("Simulation konnte nicht gestartet werden: {err}"))?;
            Ok(format!("Simulation für '{}' gestartet", project_name))
        },
        Message::SimulationActionFinished,
    )
}

fn stop_simulation_task(runtime: Arc<RuntimeManager>) -> Task<Message> {
    Task::perform(
        async move {
            runtime
                .stop_simulation()
                .await
                .map_err(|err| format!("Simulation konnte nicht gestoppt werden: {err}"))?;
            Ok("Simulation gestoppt".to_string())
        },
        Message::SimulationActionFinished,
    )
}

fn open_url_task(url: String) -> Task<Message> {
    Task::perform(
        async move {
            open_url_in_browser(&url)?;
            Ok(format!("Browser geöffnet: {}", url))
        },
        Message::PlotsWebOpened,
    )
}

fn open_url_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    cmd.spawn()
        .map(|_| ())
        .map_err(|err| format!("Browser konnte nicht geöffnet werden: {err}"))
}

fn refresh_task(runtime: Arc<RuntimeManager>) -> Task<Message> {
    Task::perform(
        async move {
            let snapshot = runtime
                .active_state()
                .await
                .map_err(|err| format!("Status konnte nicht gelesen werden: {err}"))?;

            if snapshot.is_none() {
                return Ok(RefreshData {
                    snapshot: None,
                    params: None,
                    samples: Vec::new(),
                });
            }

            let params = runtime
                .active_get_params()
                .await
                .map_err(|err| format!("Parameter konnten nicht gelesen werden: {err}"))?;
            let samples = runtime
                .active_read_plots(400)
                .await
                .map_err(|err| format!("Plotdaten konnten nicht gelesen werden: {err}"))?;

            Ok(RefreshData {
                snapshot,
                params: Some(params),
                samples,
            })
        },
        Message::Refreshed,
    )
}

fn set_param_task(
    runtime: Arc<RuntimeManager>,
    project_service: ProjectService,
    key: String,
    value: LogicParamValue,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut updates = HashMap::new();
            updates.insert(key.clone(), value);

            let params = runtime
                .active_set_params(updates.clone())
                .await
                .map_err(|err| format!("Parameter konnten nicht gesetzt werden: {err}"))?;

            if let Some(project_name) = runtime.active_project().await {
                project_service
                    .update_logic_params(&project_name, updates)
                    .map_err(|err| format!("Parameterpersistenz fehlgeschlagen: {err}"))?;
            }

            Ok(ParamApplyResult { key, params })
        },
        Message::ParamApplied,
    )
}

fn parse_communication_editor(editor: &CommunicationEditorState) -> Result<(String, u64), String> {
    let endpoint_url = editor.endpoint_url.trim().to_string();
    if endpoint_url.is_empty() {
        return Err("Kommunikation benötigt eine OPC-UA Server-Adresse".to_string());
    }

    let io_cycle_ms = editor
        .io_cycle_ms
        .trim()
        .parse::<u64>()
        .map_err(|_| "communication.io_cycle_ms muss eine ganze Zahl sein".to_string())?;
    if io_cycle_ms == 0 {
        return Err("communication.io_cycle_ms muss > 0 sein".to_string());
    }

    Ok((endpoint_url, io_cycle_ms))
}

fn save_communication_task(
    project_service: ProjectService,
    project_name: String,
    editor: CommunicationEditorState,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            let (endpoint_url, io_cycle_ms) = parse_communication_editor(&editor)?;
            cfg.communication.endpoint_url = endpoint_url;
            cfg.communication.io_cycle_ms = io_cycle_ms;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Kommunikation konnte nicht gespeichert werden: {err}"))?;

            Ok(format!("Kommunikation für '{}' gespeichert", project_name))
        },
        Message::CommunicationSaved,
    )
}

fn parse_watchdog_editor(editor: &WatchdogEditorState) -> Result<WatchdogConfig, String> {
    match editor.kind {
        WatchdogKindChoice::None => Ok(WatchdogConfig::None),
        WatchdogKindChoice::CounterMirror => {
            let read_node = editor.read_tag.trim().to_string();
            let write_node = editor.write_tag.trim().to_string();
            if read_node.is_empty() || write_node.is_empty() {
                return Err(
                    "Watchdog counter_mirror benötigt OPC-UA Read/Write Tagnamen".to_string(),
                );
            }

            let mirror_cycle_ms =
                editor.mirror_cycle_ms.trim().parse::<u64>().map_err(|_| {
                    "Watchdog mirror_cycle_ms muss eine ganze Zahl sein".to_string()
                })?;
            if mirror_cycle_ms == 0 {
                return Err("Watchdog mirror_cycle_ms muss > 0 sein".to_string());
            }

            let stale_timeout_ms =
                editor.stale_timeout_ms.trim().parse::<u64>().map_err(|_| {
                    "Watchdog stale_timeout_ms muss eine ganze Zahl sein".to_string()
                })?;
            if stale_timeout_ms == 0 {
                return Err("Watchdog stale_timeout_ms muss > 0 sein".to_string());
            }

            Ok(WatchdogConfig::CounterMirror {
                read_node,
                write_node,
                mirror_cycle_ms,
                stale_timeout_ms,
            })
        }
    }
}

fn save_watchdog_task(
    project_service: ProjectService,
    project_name: String,
    editor: WatchdogEditorState,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            cfg.watchdog = parse_watchdog_editor(&editor)?;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Watchdog konnte nicht gespeichert werden: {err}"))?;

            Ok(format!("Watchdog für '{}' gespeichert", project_name))
        },
        Message::WatchdogSaved,
    )
}

fn test_watchdog_task(
    project_service: ProjectService,
    project_name: String,
    editor: WatchdogEditorState,
) -> Task<Message> {
    Task::perform(
        async move {
            let cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            let watchdog = parse_watchdog_editor(&editor)?;

            match watchdog {
                WatchdogConfig::None => {
                    Ok("Watchdog ist auf 'none' gestellt (kein Test nötig).".to_string())
                }
                WatchdogConfig::CounterMirror {
                    read_node,
                    write_node,
                    mirror_cycle_ms,
                    stale_timeout_ms,
                } => {
                    let adapter = OpcUaClientAdapter::new(cfg.communication.endpoint_url.clone());
                    adapter.connect().await.map_err(|err| {
                        format!("Watchdog-Test: OPC-UA Verbindung fehlgeschlagen: {err}")
                    })?;

                    let read_value = adapter.read(&read_node).await.map_err(|err| {
                        format!(
                            "Watchdog-Test: Lesen von '{}' fehlgeschlagen: {err}",
                            read_node
                        )
                    })?;

                    let counter = read_value.as_f64().ok_or_else(|| {
                        format!(
                            "Watchdog-Test: Read-Tag '{}' ist kein f64-Zählerwert",
                            read_node
                        )
                    })?;

                    adapter
                        .write(&write_node, SignalValue::F64(counter))
                        .await
                        .map_err(|err| {
                            format!(
                                "Watchdog-Test: Schreiben auf '{}' fehlgeschlagen: {err}",
                                write_node
                            )
                        })?;

                    Ok(format!(
                        "Watchdog-Test OK: '{}' ({counter:.3}) -> '{}' | cycle={}ms timeout={}ms",
                        read_node, write_node, mirror_cycle_ms, stale_timeout_ms
                    ))
                }
            }
        },
        Message::WatchdogTested,
    )
}

fn save_variables_task(
    project_service: ProjectService,
    project_name: String,
    mv_rows: Vec<VariableNameRow>,
    cv_rows: Vec<VariableNameRow>,
    dv_rows: Vec<VariableNameRow>,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            cfg.variables = build_variable_catalog(&mv_rows, &cv_rows, &dv_rows)?;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Variablen konnten nicht gespeichert werden: {err}"))?;

            Ok(format!("Variablen für '{}' gespeichert", project_name))
        },
        Message::VariablesSaved,
    )
}

fn test_io_read_task(
    project_service: ProjectService,
    project_name: String,
    endpoint_url_from_editor: String,
    mv_rows: Vec<MvIoEditorRow>,
    cv_rows: Vec<CvIoEditorRow>,
    dv_rows: Vec<DvIoEditorRow>,
) -> Task<Message> {
    Task::perform(
        async move {
            let cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            let endpoint_url = if endpoint_url_from_editor.trim().is_empty() {
                cfg.communication.endpoint_url
            } else {
                endpoint_url_from_editor.trim().to_string()
            };

            let adapter = OpcUaClientAdapter::new(endpoint_url.clone());
            adapter.connect().await.map_err(|err| {
                format!("I/O Read-Test: OPC-UA Verbindung fehlgeschlagen ({endpoint_url}): {err}")
            })?;

            let mut mv_values = Vec::with_capacity(mv_rows.len());
            for row in &mv_rows {
                let mv_write = read_optional_tag(&adapter, row.mv_write.trim()).await;
                let mv_read = read_optional_tag(&adapter, row.mv_read.trim()).await;
                let hl = read_optional_tag(&adapter, row.hl_opc.trim()).await;
                let ll = read_optional_tag(&adapter, row.ll_opc.trim()).await;
                let enable = read_optional_tag(&adapter, row.enable.trim()).await;
                mv_values.push(MvReadValues {
                    mv_write,
                    mv_read,
                    hl,
                    ll,
                    enable,
                });
            }

            let mut cv_values = Vec::with_capacity(cv_rows.len());
            for row in &cv_rows {
                let soll = read_optional_tag(&adapter, row.soll.trim()).await;
                let ist = read_optional_tag(&adapter, row.ist.trim()).await;
                let sollband = read_optional_tag(&adapter, row.sollband_opc.trim()).await;
                let enable = read_optional_tag(&adapter, row.enable.trim()).await;
                cv_values.push(CvReadValues {
                    soll,
                    ist,
                    sollband,
                    enable,
                });
            }

            let mut dv_values = Vec::with_capacity(dv_rows.len());
            for row in &dv_rows {
                let read = read_optional_tag(&adapter, row.read.trim()).await;
                let enable = read_optional_tag(&adapter, row.enable.trim()).await;
                dv_values.push(DvReadValues { read, enable });
            }

            Ok(IoReadTestResult {
                mv_values,
                cv_values,
                dv_values,
            })
        },
        Message::IoReadTested,
    )
}

fn test_io_write_all_task(
    project_service: ProjectService,
    project_name: String,
    endpoint_url_from_editor: String,
    mv_rows: Vec<MvIoEditorRow>,
    cv_rows: Vec<CvIoEditorRow>,
) -> Task<Message> {
    Task::perform(
        async move {
            let cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            let endpoint_url = if endpoint_url_from_editor.trim().is_empty() {
                cfg.communication.endpoint_url
            } else {
                endpoint_url_from_editor.trim().to_string()
            };

            let adapter = OpcUaClientAdapter::new(endpoint_url.clone());
            adapter.connect().await.map_err(|err| {
                format!("test_write: OPC-UA Verbindung fehlgeschlagen ({endpoint_url}): {err}")
            })?;

            let mut write_count = 0usize;

            for row in &mv_rows {
                let address = row.mv_write.trim();
                let raw_value = row.test_write.trim();
                if address.is_empty() || raw_value.is_empty() {
                    continue;
                }
                let value = parse_test_write_value(raw_value)?;
                adapter.write(address, value).await.map_err(|err| {
                    format!(
                        "test_write: Schreiben auf '{}' fehlgeschlagen: {err}",
                        address
                    )
                })?;
                write_count += 1;
            }

            for row in &cv_rows {
                let address = row.soll.trim();
                let raw_value = row.test_write.trim();
                if address.is_empty() || raw_value.is_empty() {
                    continue;
                }
                let value = parse_test_write_value(raw_value)?;
                adapter.write(address, value).await.map_err(|err| {
                    format!(
                        "test_write: Schreiben auf '{}' fehlgeschlagen: {err}",
                        address
                    )
                })?;
                write_count += 1;
            }

            Ok(format!(
                "test_write abgeschlossen: {} Schreibvorgänge",
                write_count
            ))
        },
        Message::IoWriteTested,
    )
}

fn save_io_definition_task(
    project_service: ProjectService,
    project_name: String,
    mv_name_rows: Vec<VariableNameRow>,
    cv_name_rows: Vec<VariableNameRow>,
    dv_name_rows: Vec<VariableNameRow>,
    mv_rows: Vec<MvIoEditorRow>,
    cv_rows: Vec<CvIoEditorRow>,
    dv_rows: Vec<DvIoEditorRow>,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            let vars = build_variable_catalog(&mv_name_rows, &cv_name_rows, &dv_name_rows)?;
            cfg.variables = vars.clone();
            cfg.io_definition = build_io_definition(&vars, &mv_rows, &cv_rows, &dv_rows)?;
            cfg.rebuild_runtime_io_from_definition()
                .map_err(|err| format!("I/O-Ableitung fehlgeschlagen: {err}"))?;
            sync_io_initial_value_online_params(&mut cfg);

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("I/O Definition konnte nicht gespeichert werden: {err}"))?;

            Ok(format!("I/O Definition für '{}' gespeichert", project_name))
        },
        Message::IoDefinitionSaved,
    )
}

fn save_model_task(
    project_service: ProjectService,
    project_name: String,
    editor: ModelEditorState,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            cfg.model = build_model_config_from_editor(&editor, &cfg.variables)?;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Model konnte nicht gespeichert werden: {err}"))?;

            Ok(format!("Model for '{}' saved", project_name))
        },
        Message::ModelSaved,
    )
}

fn save_simulation_task(
    project_service: ProjectService,
    project_name: String,
    editor: SimulationEditorState,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            cfg.simulation =
                build_simulation_config_from_editor(&editor, &cfg.variables, cfg.model.as_ref())?;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Simulation konnte nicht gespeichert werden: {err}"))?;

            Ok(format!("Simulation für '{}' gespeichert", project_name))
        },
        Message::SimulationSaved,
    )
}

fn save_simulation_live_scalar_task(
    runtime: Arc<RuntimeManager>,
    project_service: ProjectService,
    project_name: String,
    field: SimulationLiveScalarField,
    value: f64,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            cfg.normalize_simulation_config();
            match &field {
                SimulationLiveScalarField::CvSetpoint(name) => {
                    cfg.simulation.cv_setpoints.insert(name.clone(), value);
                }
                SimulationLiveScalarField::CvNoiseStd(name) => {
                    cfg.simulation.cv_noise_std.insert(name.clone(), value);
                }
                SimulationLiveScalarField::MvNoiseStd(name) => {
                    cfg.simulation.mv_noise_std.insert(name.clone(), value);
                }
                SimulationLiveScalarField::DvDisturbance(name) => {
                    cfg.simulation.dv_disturbances.insert(name.clone(), value);
                }
                SimulationLiveScalarField::CvOffset(name) => {
                    cfg.simulation.cv_offsets.insert(name.clone(), value);
                }
            }
            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| {
                    format!("Live-Simulationswert konnte nicht gespeichert werden: {err}")
                })?;
            runtime
                .apply_simulation_live_config(cfg.simulation.clone())
                .await
                .map_err(|err| {
                    format!("Live-Simulationswert konnte nicht übernommen werden: {err}")
                })?;

            Ok(format!("Live-Simulationswert aktualisiert"))
        },
        Message::SimulationLiveUpdated,
    )
}

fn save_simulation_live_matrix_task(
    runtime: Arc<RuntimeManager>,
    project_service: ProjectService,
    project_name: String,
    matrix: MatrixKey,
    row_index: usize,
    col_index: usize,
    value: f64,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            cfg.normalize_simulation_config();
            let cells = match matrix {
                MatrixKey::A => &mut cfg.simulation.a_scale_pct,
                MatrixKey::B => &mut cfg.simulation.b_scale_pct,
                MatrixKey::C | MatrixKey::D => {
                    return Err("Nur A/B-Skalierung ist für die Simulation erlaubt".to_string());
                }
            };
            let Some(row) = cells.get_mut(row_index) else {
                return Err("Live-Skalierungszeile außerhalb des Modells".to_string());
            };
            let Some(cell) = row.get_mut(col_index) else {
                return Err("Live-Skalierungsspalte außerhalb des Modells".to_string());
            };
            *cell = value;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Live-Skalierung konnte nicht gespeichert werden: {err}"))?;
            runtime
                .apply_simulation_live_config(cfg.simulation.clone())
                .await
                .map_err(|err| format!("Live-Skalierung konnte nicht übernommen werden: {err}"))?;

            Ok("Live-Skalierung aktualisiert".to_string())
        },
        Message::SimulationLiveUpdated,
    )
}

fn build_model_config_from_editor(
    editor: &ModelEditorState,
    vars: &VariableCatalogConfig,
) -> Result<Option<ModelConfig>, String> {
    match editor.selected_kind {
        ModelKindChoice::None => Ok(None),
        ModelKindChoice::LinearStateSpace => {
            if !vars.has_any() {
                return Err(
                    "Define MV/CV/DV names before saving a linear state-space model".to_string(),
                );
            }

            let state_count = parse_state_count_value(&editor.state_count)?;
            let u_order = model_input_order(vars);
            let y_order = vars.cvs.clone();

            let a = parse_numeric_matrix("A", &editor.a_cells, state_count, state_count)?;
            let b = parse_numeric_matrix("B", &editor.b_cells, state_count, u_order.len())?;
            let c = parse_numeric_matrix("C", &editor.c_cells, y_order.len(), state_count)?;
            let d = parse_numeric_matrix("D", &editor.d_cells, y_order.len(), u_order.len())?;

            Ok(Some(ModelConfig::LinearStateSpace {
                model: LinearStateSpaceModelConfig {
                    state_count,
                    u_order,
                    y_order,
                    a,
                    b,
                    c,
                    d,
                },
            }))
        }
    }
}

fn build_simulation_config_from_editor(
    editor: &SimulationEditorState,
    _vars: &VariableCatalogConfig,
    model: Option<&ModelConfig>,
) -> Result<SimulationConfig, String> {
    let linear_model = model.and_then(ModelConfig::linear_state_space);
    let (a_rows, a_cols, b_rows, b_cols) = match linear_model {
        Some(model) => (
            model.state_count,
            model.state_count,
            model.state_count,
            model.u_order.len(),
        ),
        None => (0, 0, 0, 0),
    };

    Ok(SimulationConfig {
        speed: editor.speed.into(),
        cv_setpoints: parse_named_value_rows("cv setpoint", &editor.cv_setpoints, false)?,
        cv_initials: parse_named_value_rows("cv initial", &editor.cv_initials, false)?,
        mv_initials: parse_named_value_rows("mv initial", &editor.mv_initials, false)?,
        dv_initials: parse_named_value_rows("dv initial", &editor.dv_initials, false)?,
        cv_noise_std: parse_named_value_rows("cv noise", &editor.cv_noise_std, true)?,
        mv_noise_std: parse_named_value_rows("mv noise", &editor.mv_noise_std, true)?,
        dv_disturbances: parse_named_value_rows("dv disturbance", &editor.dv_disturbances, false)?,
        cv_offsets: parse_named_value_rows("cv offset", &editor.cv_offsets, false)?,
        a_scale_pct: parse_numeric_matrix("A scale", &editor.a_scale_cells, a_rows, a_cols)?,
        b_scale_pct: parse_numeric_matrix("B scale", &editor.b_scale_cells, b_rows, b_cols)?,
    })
}

fn parse_numeric_matrix(
    name: &str,
    cells: &[Vec<String>],
    expected_rows: usize,
    expected_cols: usize,
) -> Result<Vec<Vec<f64>>, String> {
    if cells.len() != expected_rows {
        return Err(format!(
            "{} matrix has {} rows, expected {}",
            name,
            cells.len(),
            expected_rows
        ));
    }

    let mut out = Vec::with_capacity(expected_rows);
    for (row_index, row) in cells.iter().enumerate() {
        if row.len() != expected_cols {
            return Err(format!(
                "{} matrix row {} has {} columns, expected {}",
                name,
                row_index + 1,
                row.len(),
                expected_cols
            ));
        }

        let mut numeric_row = Vec::with_capacity(expected_cols);
        for (col_index, value) in row.iter().enumerate() {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(format!(
                    "{}[{}, {}] requires a numeric value",
                    name,
                    row_index + 1,
                    col_index + 1
                ));
            }

            let parsed = trimmed.parse::<f64>().map_err(|_| {
                format!(
                    "{}[{}, {}] is not a valid number: '{}'",
                    name,
                    row_index + 1,
                    col_index + 1,
                    trimmed
                )
            })?;

            if !parsed.is_finite() {
                return Err(format!(
                    "{}[{}, {}] must be finite",
                    name,
                    row_index + 1,
                    col_index + 1
                ));
            }

            numeric_row.push(parsed);
        }
        out.push(numeric_row);
    }

    Ok(out)
}

fn save_logic_task(
    project_service: ProjectService,
    project_name: String,
    editor: LogicEditorState,
    existing_params: HashMap<String, LogicParamValue>,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            cfg.logic.kind = editor.selected_kind.into();
            cfg.logic.custom_rust.strategy_name = editor.custom_strategy_name.trim().to_string();
            cfg.logic.custom_rust.source_path = editor.custom_source_path.trim().to_string();
            cfg.logic.custom_rust.artifact_path = editor.custom_artifact_path.trim().to_string();

            let mut pairs = Vec::new();
            for row in editor.p_pairs {
                let cv = row.cv.trim().to_string();
                let mv = row.mv.trim().to_string();
                if cv.is_empty() && mv.is_empty() {
                    continue;
                }
                if cv.is_empty() || mv.is_empty() {
                    return Err("Jedes P-Paar braucht CV und MV".to_string());
                }
                let kp = row
                    .kp
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("Ungültiges kp für Paar {} -> {}", cv, mv))?;
                let bias = row
                    .bias
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("Ungültiges bias für Paar {} -> {}", cv, mv))?;
                pairs.push(PControllerPairConfig { cv, mv, kp, bias });
            }
            if cfg.logic.kind == LogicKind::PController && pairs.is_empty() {
                return Err("Für P-Regler bitte mindestens ein CV->MV Paar anlegen".to_string());
            }
            cfg.logic.p_controller.pairs = pairs.clone();
            cfg.logic.params = existing_params;
            for pair in &pairs {
                cfg.logic.params.insert(
                    format!("p.{}.{}.kp", pair.cv, pair.mv),
                    LogicParamValue::F64(pair.kp),
                );
                cfg.logic.params.insert(
                    format!("p.{}.{}.bias", pair.cv, pair.mv),
                    LogicParamValue::F64(pair.bias),
                );
            }
            cfg.sync_active_online_params();

            if cfg.logic.kind == LogicKind::CustomRust
                && !cfg.logic.custom_rust.is_compatible_with(&cfg.io)
            {
                return Err(
                    "Eigene Regellogik ist aktuell nicht kompatibel mit der I/O-Definition"
                        .to_string(),
                );
            }

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Regellogik konnte nicht gespeichert werden: {err}"))?;
            Ok(format!("Regellogik für '{}' gespeichert", project_name))
        },
        Message::LogicSaved,
    )
}

fn generate_logic_template_task(
    project_service: ProjectService,
    project_name: String,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            if cfg.io.outputs.is_empty() {
                return Err("I/O ist leer. Bitte zuerst 'I/O definieren' abschließen.".to_string());
            }

            let project_dir = project_service.project_dir(&project_name);
            let strategy_name = format!("{}_custom_logic", sanitize_crate_name(&project_name));
            let strategy_root = project_dir.join("logic").join("custom_strategy");
            let src_dir = strategy_root.join("src");
            fs::create_dir_all(&src_dir)
                .map_err(|err| format!("Template-Ordner konnte nicht erstellt werden: {err}"))?;

            let cargo_toml = render_custom_cargo_toml(&strategy_name);
            fs::write(strategy_root.join("Cargo.toml"), cargo_toml)
                .map_err(|err| format!("Cargo.toml konnte nicht geschrieben werden: {err}"))?;

            let required_inputs: Vec<String> = cfg
                .io
                .inputs
                .iter()
                .map(|point| point.name.clone())
                .collect();
            let required_outputs: Vec<String> = cfg
                .io
                .outputs
                .iter()
                .map(|point| point.name.clone())
                .collect();
            fs::write(
                src_dir.join("lib.rs"),
                render_custom_template(&required_inputs, &required_outputs),
            )
            .map_err(|err| format!("Template lib.rs konnte nicht geschrieben werden: {err}"))?;

            fs::write(
                src_dir.join("generated_dispatch.rs"),
                render_generated_dispatch(&[]),
            )
            .map_err(|err| {
                format!("generated_dispatch.rs konnte nicht geschrieben werden: {err}")
            })?;

            let artifact_name = format!("{}{}{}", dylib_prefix(), strategy_name, dylib_extension());
            let source_rel = "logic/custom_strategy/src/lib.rs".to_string();
            let artifact_rel = format!("logic/custom_strategy/target/release/{}", artifact_name);

            cfg.logic.custom_rust.strategy_name = strategy_name;
            cfg.logic.custom_rust.source_path = source_rel;
            cfg.logic.custom_rust.artifact_path = artifact_rel;
            cfg.logic.custom_rust.parameter_schema.clear();
            cfg.logic.custom_rust.required_inputs = required_inputs;
            cfg.logic.custom_rust.required_outputs = required_outputs;
            cfg.logic.custom_rust.compatible = false;
            cfg.logic.custom_rust.last_build_message =
                "Template erzeugt. Bitte 'Strategie prüfen/builden' ausführen.".to_string();
            if cfg.logic.kind == LogicKind::CustomRust {
                cfg.logic.kind = LogicKind::PController;
            }

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| {
                    format!("Template-Konfiguration konnte nicht gespeichert werden: {err}")
                })?;

            Ok(format!(
                "Template für '{}' erzeugt unter {}",
                project_name,
                strategy_root.display()
            ))
        },
        Message::LogicTemplateGenerated,
    )
}

fn build_custom_logic_task(project_service: ProjectService, project_name: String) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            let project_dir = project_service.project_dir(&project_name);
            let strategy_root = project_dir.join("logic").join("custom_strategy");
            let source_rel = if cfg.logic.custom_rust.source_path.trim().is_empty() {
                "logic/custom_strategy/src/lib.rs".to_string()
            } else {
                cfg.logic.custom_rust.source_path.clone()
            };
            let source_path = project_dir.join(&source_rel);
            if !source_path.exists() {
                return Err(format!(
                    "Custom source nicht gefunden: {}",
                    source_path.display()
                ));
            }

            let source = fs::read_to_string(&source_path)
                .map_err(|err| format!("Custom source konnte nicht gelesen werden: {err}"))?;
            let schema = parse_custom_parameter_schema_from_source(&source)
                .map_err(|err| format!("Signatur ungültig: {err}"))?;

            let generated_dispatch_path = source_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("generated_dispatch.rs");
            fs::write(&generated_dispatch_path, render_generated_dispatch(&schema)).map_err(
                |err| format!("generated_dispatch.rs konnte nicht geschrieben werden: {err}"),
            )?;

            let output = Command::new("cargo")
                .arg("build")
                .arg("--release")
                .arg("--manifest-path")
                .arg(strategy_root.join("Cargo.toml"))
                .output()
                .map_err(|err| format!("cargo build konnte nicht gestartet werden: {err}"))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                cfg.logic.custom_rust.compatible = false;
                cfg.logic.custom_rust.last_build_message =
                    first_line_or(&stderr, "Build fehlgeschlagen");
                project_service
                    .save_config(&project_name, &cfg)
                    .map_err(|err| {
                        format!(
                            "Konfiguration konnte nach Build-Fehler nicht gespeichert werden: {err}"
                        )
                    })?;
                return Err(format!(
                    "Build fehlgeschlagen: {}",
                    first_line_or(&stderr, "Unbekannter Fehler")
                ));
            }

            let strategy_name = if cfg.logic.custom_rust.strategy_name.trim().is_empty() {
                format!("{}_custom_logic", sanitize_crate_name(&project_name))
            } else {
                cfg.logic.custom_rust.strategy_name.clone()
            };
            let artifact_name = format!("{}{}{}", dylib_prefix(), strategy_name, dylib_extension());
            let artifact_rel = if cfg.logic.custom_rust.artifact_path.trim().is_empty() {
                format!("logic/custom_strategy/target/release/{}", artifact_name)
            } else {
                cfg.logic.custom_rust.artifact_path.clone()
            };
            let artifact_path = project_dir.join(&artifact_rel);
            if !artifact_path.exists() {
                return Err(format!(
                    "Build erfolgreich, aber Artefakt fehlt: {}",
                    artifact_path.display()
                ));
            }

            let contract = inspect_custom_library_contract(&artifact_path).map_err(|err| {
                format!("Build ok, aber Metadaten aus cdylib konnten nicht gelesen werden: {err}")
            })?;

            if schema_signature(&schema) != schema_signature(&contract.parameter_schema) {
                cfg.logic.custom_rust.compatible = false;
                cfg.logic.custom_rust.last_build_message =
                    "Build erfolgreich, aber Parameter-Schema in cdylib passt nicht zur control-Signatur".to_string();
                project_service
                    .save_config(&project_name, &cfg)
                    .map_err(|err| {
                        format!(
                            "Konfiguration konnte nach Schema-Fehler nicht gespeichert werden: {err}"
                        )
                    })?;
                return Err(
                    "Build erfolgreich, aber Parameter-Schema in cdylib passt nicht zur control-Signatur".to_string(),
                );
            }

            let expected_inputs: Vec<String> = cfg
                .io
                .inputs
                .iter()
                .map(|point| point.name.clone())
                .collect();
            let expected_outputs: Vec<String> = cfg
                .io
                .outputs
                .iter()
                .map(|point| point.name.clone())
                .collect();
            let strict_io_match = io_signature(&expected_inputs)
                == io_signature(&contract.required_inputs)
                && io_signature(&expected_outputs) == io_signature(&contract.required_outputs);

            cfg.logic.custom_rust.strategy_name = strategy_name;
            cfg.logic.custom_rust.source_path = source_rel;
            cfg.logic.custom_rust.artifact_path = artifact_rel;
            cfg.logic.custom_rust.parameter_schema = contract.parameter_schema.clone();
            cfg.logic.custom_rust.required_inputs = contract.required_inputs.clone();
            cfg.logic.custom_rust.required_outputs = contract.required_outputs.clone();
            cfg.logic.custom_rust.compatible = strict_io_match;
            cfg.logic.custom_rust.last_build_message = if strict_io_match {
                "Build erfolgreich".to_string()
            } else {
                format!(
                    "Build erfolgreich, aber I/O-Mismatch: {}",
                    summarize_io_mismatch(
                        &expected_inputs,
                        &expected_outputs,
                        &contract.required_inputs,
                        &contract.required_outputs
                    )
                )
            };

            cfg.sync_active_online_params();

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Custom-Logik konnte nicht gespeichert werden: {err}"))?;

            if strict_io_match {
                Ok(format!(
                    "Custom-Strategie gebaut und kompatibel für '{}'",
                    project_name
                ))
            } else {
                Ok(format!(
                    "Custom-Strategie gebaut, aber I/O-Mismatch für '{}'",
                    project_name,
                ))
            }
        },
        Message::CustomLogicBuilt,
    )
}

fn run_control_function_tests_task(
    project_service: ProjectService,
    project_name: String,
    test_state: ControlFunctionTestState,
) -> Task<Message> {
    Task::perform(
        async move {
            let cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;
            let selections = build_control_function_sweep_selections(&test_state)?;
            let project_dir = project_service.project_dir(&project_name);
            run_control_function_tests(&cfg, &project_dir, &selections)
                .map_err(|err| format!("Control Function Tests fehlgeschlagen: {err}"))
        },
        Message::ControlFunctionTestsFinished,
    )
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
        parts.push(format!("fehlende inputs [{}]", missing_in.join(", ")));
    }
    if !extra_in.is_empty() {
        parts.push(format!("zusätzliche inputs [{}]", extra_in.join(", ")));
    }
    if !missing_out.is_empty() {
        parts.push(format!("fehlende outputs [{}]", missing_out.join(", ")));
    }
    if !extra_out.is_empty() {
        parts.push(format!("zusätzliche outputs [{}]", extra_out.join(", ")));
    }

    if parts.is_empty() {
        "unbekannte Abweichung".to_string()
    } else {
        parts.join("; ")
    }
}

fn render_custom_cargo_toml(crate_name: &str) -> String {
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

fn render_custom_template(required_inputs: &[String], required_outputs: &[String]) -> String {
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
    // Aktueller Runtime-State.
    pub state: RuntimeState,
    // Aktueller Input-Snapshot (alle verfügbaren Eingänge).
    pub inputs: HashMap<String, SignalValue>,
    // Aktuelle Online-Parameter.
    pub params: HashMap<String, ParamValue>,
    // Historie der Inputs (ältest -> neuest).
    #[serde(default)]
    pub input_history: HashMap<String, Vec<SignalValue>>,
    // Historie der Outputs (ältest -> neuest).
    #[serde(default)]
    pub output_history: HashMap<String, Vec<SignalValue>>,
    // Historie der Online-Parameter (ältest -> neuest).
    #[serde(default)]
    pub param_history: HashMap<String, Vec<ParamValue>>,
}}

#[derive(Debug, Clone, Default)]
pub struct SmartApcOutputs {{
    pub values: HashMap<String, SignalValue>,
    // Zusätzliche User-Logs aus der Regelstrategie.
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
    // TEMPLATE HINWEISE
    // 1) ctx.inputs enthält den aktuellen Zykluswert je Signal.
    // 2) ctx.input_history/output_history/param_history enthalten Verlaufswerte (ältest -> neuest).
    // 3) Zusätzliche Parameter können als weitere Funktionsargumente ergänzt werden:
    //    z. B. `pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs, gain: f64)`.
    //    Diese werden automatisch als Online-Parameter geführt.
    // 4) Alle benötigten `mv.*.write` Outputs setzen.
    // 5) Eigene Logs können über out.log(...), out.log_value(...), out.log_values(...) erzeugt werden.
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

fn render_generated_dispatch(
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

fn signal_value_to_test_string(value: SignalValue) -> String {
    match value {
        SignalValue::F64(v) => format!("{v:.6}"),
        SignalValue::Bool(v) => v.to_string(),
    }
}

async fn read_optional_tag(adapter: &OpcUaClientAdapter, address: &str) -> String {
    if address.trim().is_empty() {
        return String::new();
    }

    match adapter.read(address).await {
        Ok(value) => signal_value_to_test_string(value),
        Err(err) => format!("ERR: {err}"),
    }
}

fn parse_test_write_value(raw: &str) -> Result<SignalValue, String> {
    let value = raw
        .parse::<f64>()
        .map_err(|_| format!("test_write: '{}' ist kein gültiger f64-Wert", raw))?;
    Ok(SignalValue::F64(value))
}

fn optional_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn build_variable_catalog(
    mv_rows: &[VariableNameRow],
    cv_rows: &[VariableNameRow],
    dv_rows: &[VariableNameRow],
) -> Result<VariableCatalogConfig, String> {
    let mut vars = VariableCatalogConfig {
        mvs: collect_variable_names(mv_rows),
        cvs: collect_variable_names(cv_rows),
        dvs: collect_variable_names(dv_rows),
    };

    vars.validate()
        .map_err(|err| format!("Variablen ungültig: {err}"))?;

    if !vars.has_any() {
        return Err("Bitte mindestens eine Variable (MV/CV/DV) definieren".to_string());
    }

    vars.mvs.sort();
    vars.cvs.sort();
    vars.dvs.sort();
    Ok(vars)
}

fn build_io_definition(
    vars: &VariableCatalogConfig,
    mv_rows: &[MvIoEditorRow],
    cv_rows: &[CvIoEditorRow],
    dv_rows: &[DvIoEditorRow],
) -> Result<crate::project_config::IoDefinitionConfig, String> {
    let mut mvs = Vec::new();
    for name in &vars.mvs {
        let row = mv_rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("MV '{}' fehlt in I/O Definition", name))?;

        let mv_write = row.mv_write.trim().to_string();
        let mv_read = row.mv_read.trim().to_string();
        let hl = if row.hl.trim().is_empty() {
            None
        } else {
            Some(
                row.hl
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("MV '{}' HL ist keine gültige Zahl", name))?,
            )
        };
        let ll = if row.ll.trim().is_empty() {
            None
        } else {
            Some(
                row.ll
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("MV '{}' LL ist keine gültige Zahl", name))?,
            )
        };
        if let (Some(ll), Some(hl)) = (ll, hl) {
            if ll > hl {
                return Err(format!("MV '{}' hat LL > HL", name));
            }
        }

        mvs.push(MvIoDefinition {
            name: name.clone(),
            mv_write,
            mv_read,
            hl_opc: optional_trimmed(&row.hl_opc),
            ll_opc: optional_trimmed(&row.ll_opc),
            hl,
            ll,
            enable: optional_trimmed(&row.enable),
        });
    }

    let mut cvs = Vec::new();
    for name in &vars.cvs {
        let row = cv_rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("CV '{}' fehlt in I/O Definition", name))?;

        let soll = row.soll.trim().to_string();
        let ist = row.ist.trim().to_string();

        let sollband_value = if row.sollband_value.trim().is_empty() {
            None
        } else {
            Some(
                row.sollband_value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("CV '{}' Sollband-Wert ist keine gültige Zahl", name))?,
            )
        };
        let sollband_opc = optional_trimmed(&row.sollband_opc);

        cvs.push(CvIoDefinition {
            name: name.clone(),
            soll,
            ist,
            sollband_opc,
            sollband_value,
            enable: optional_trimmed(&row.enable),
        });
    }

    let mut dvs = Vec::new();
    for name in &vars.dvs {
        let row = dv_rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("DV '{}' fehlt in I/O Definition", name))?;
        let read = row.read.trim().to_string();
        dvs.push(DvIoDefinition {
            name: name.clone(),
            read,
            enable: optional_trimmed(&row.enable),
        });
    }

    let io_definition = crate::project_config::IoDefinitionConfig { mvs, cvs, dvs };
    io_definition
        .validate(vars)
        .map_err(|err| format!("I/O Definition ungültig: {err}"))?;
    if !io_definition.is_complete_for(vars) {
        return Err("I/O Definition ist unvollständig".to_string());
    }
    Ok(io_definition)
}

fn sync_io_initial_value_online_params(cfg: &mut crate::project_config::ProjectConfig) {
    // Keep only parameters that are relevant for the currently active logic.
    // Static I/O initial values (without OPC address) are included by sync_active_online_params.
    cfg.sync_active_online_params();
}

fn start_web_task(
    project_service: ProjectService,
    runtime: Arc<RuntimeManager>,
    executable_path: PathBuf,
    bind: SocketAddr,
) -> Task<Message> {
    Task::perform(
        async move {
            let state = AppState {
                project_service,
                runtime,
                executable_path,
            };

            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .map_err(|err| format!("Web API konnte nicht an {} binden: {}", bind, err))?;

            tokio::spawn(async move {
                if let Err(err) = axum::serve(listener, build_router(state)).await {
                    tracing::error!(error = %err, "embedded web server failed");
                }
            });

            Ok(format!("Web API läuft auf http://{}", bind))
        },
        Message::WebStarted,
    )
}

fn build_plot_lines(samples: &[SignalSample], width: usize) -> Vec<String> {
    let mut numeric_series: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut boolean_series: BTreeMap<String, Vec<bool>> = BTreeMap::new();

    for sample in samples {
        let key = format!("{}::{}", sample.direction, sample.signal);
        match sample.value {
            SignalValue::F64(v) => numeric_series.entry(key).or_default().push(v),
            SignalValue::Bool(v) => boolean_series.entry(key).or_default().push(v),
        }
    }

    let mut lines = Vec::new();
    for (key, values) in numeric_series {
        let plot = ascii_plot(&values, width);
        let last = values.last().copied().unwrap_or_default();
        lines.push(format!("{:24} | {} | last={:.4}", key, plot, last));
    }

    for (key, values) in boolean_series {
        let plot = bool_plot(&values, width);
        let last = values.last().copied().unwrap_or(false);
        lines.push(format!("{:24} | {} | last={}", key, plot, i32::from(last)));
    }

    lines
}

fn ascii_plot(values: &[f64], width: usize) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }

    const SCALE: &[char] = &['.', ':', '-', '=', '+', '*', '#', '%', '@'];
    let series: Vec<f64> = if values.len() > width {
        values[values.len() - width..].to_vec()
    } else {
        values.to_vec()
    };

    let min = series.iter().copied().fold(f64::INFINITY, f64::min);
    let max = series.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).abs();
    let mut out = String::with_capacity(series.len());

    for value in series {
        let idx = if span < f64::EPSILON {
            SCALE.len() / 2
        } else {
            let norm = ((value - min) / span).clamp(0.0, 1.0);
            (norm * (SCALE.len() as f64 - 1.0)).round() as usize
        };
        out.push(SCALE[idx]);
    }

    out
}

fn bool_plot(values: &[bool], width: usize) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let series: Vec<bool> = if values.len() > width {
        values[values.len() - width..].to_vec()
    } else {
        values.to_vec()
    };
    let mut out = String::with_capacity(series.len());
    for value in series {
        out.push(if value { '1' } else { '0' });
    }
    out
}

fn build_control_function_sweep_selections(
    state: &ControlFunctionTestState,
) -> Result<Vec<SweepSelection>, String> {
    let mut selections = Vec::new();

    for row in &state.input_rows {
        selections.push(build_control_function_selection(
            crate::control_function_tests::SweepDimensionKind::Input,
            row,
        )?);
    }
    for row in &state.param_rows {
        selections.push(build_control_function_selection(
            crate::control_function_tests::SweepDimensionKind::Parameter,
            row,
        )?);
    }

    Ok(selections)
}

fn build_control_function_selection(
    kind: crate::control_function_tests::SweepDimensionKind,
    row: &ControlFunctionSweepRowState,
) -> Result<SweepSelection, String> {
    let numeric = if row.value_kind == SweepValueKind::F64 && row.enabled {
        let min = row
            .min
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("Sweep '{}' min ist keine gültige Zahl", row.name))?;
        let max = row
            .max
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("Sweep '{}' max ist keine gültige Zahl", row.name))?;
        let samples = row
            .samples
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("Sweep '{}' samples ist keine gültige Ganzzahl", row.name))?;
        if samples == 0 {
            return Err(format!("Sweep '{}' braucht mindestens 1 Sample", row.name));
        }

        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        Some(NumericSweepConfig { min, max, samples })
    } else {
        None
    };

    Ok(SweepSelection {
        kind,
        name: row.name.clone(),
        enabled: row.enabled,
        numeric,
    })
}

fn format_sweep_mode(mode: SweepExecutionMode) -> &'static str {
    match mode {
        SweepExecutionMode::FullFactorial => "vollfaktoriell",
        SweepExecutionMode::SingleAxisFallback => "Einzel-Sweeps",
    }
}

fn format_case_preview(case: &crate::control_function_tests::CasePreview) -> String {
    format!("{} | {} | {} us", case.label, case.status, case.duration_us)
}

fn format_test_numeric(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
    }
}

fn parse_state_count_value(raw: &str) -> Result<usize, String> {
    let value = raw
        .trim()
        .parse::<usize>()
        .map_err(|_| "Number of states must be a positive integer".to_string())?;
    if value == 0 {
        return Err("Number of states must be greater than 0".to_string());
    }
    Ok(value)
}

fn model_input_order(vars: &VariableCatalogConfig) -> Vec<String> {
    vars.mvs
        .iter()
        .cloned()
        .chain(vars.dvs.iter().cloned())
        .collect()
}

fn build_state_labels(count: usize) -> Vec<String> {
    (1..=count).map(|index| format!("x{index}")).collect()
}

fn format_state_vector(count: usize) -> String {
    format_named_vector(&build_state_labels(count))
}

fn format_named_vector(names: &[String]) -> String {
    if names.is_empty() {
        "(empty)".to_string()
    } else {
        names.join(", ")
    }
}

fn build_model_preview(
    editor: &ModelEditorState,
    vars: &VariableCatalogConfig,
) -> Option<ModelConfig> {
    build_model_config_from_editor(editor, vars).ok().flatten()
}

fn matrix_to_string_cells(matrix: &[Vec<f64>]) -> Vec<Vec<String>> {
    matrix
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| format_test_numeric(*value))
                .collect()
        })
        .collect()
}

fn resize_string_matrix(matrix: Vec<Vec<String>>, rows: usize, cols: usize) -> Vec<Vec<String>> {
    let mut resized = vec![vec![String::from("0.0"); cols]; rows];

    for (row_index, row) in matrix.into_iter().enumerate().take(rows) {
        for (col_index, value) in row.into_iter().enumerate().take(cols) {
            resized[row_index][col_index] = value;
        }
    }

    resized
}

fn fill_empty_cells(cells: &mut [Vec<String>], value: &str) {
    for row in cells {
        for cell in row {
            if cell.trim().is_empty() {
                *cell = value.to_string();
            }
        }
    }
}

fn named_value_rows(
    names: &[String],
    values: &HashMap<String, f64>,
) -> Vec<SimulationNamedValueRow> {
    names
        .iter()
        .map(|name| {
            SimulationNamedValueRow::from_named_value(
                name.clone(),
                values.get(name).copied().unwrap_or(0.0),
            )
        })
        .collect()
}

fn sync_named_value_rows(rows: &mut Vec<SimulationNamedValueRow>, names: &[String]) {
    let existing: HashMap<_, _> = rows
        .iter()
        .map(|row| (row.name.clone(), row.value.clone()))
        .collect();
    *rows = names
        .iter()
        .map(|name| SimulationNamedValueRow {
            name: name.clone(),
            value: existing
                .get(name)
                .cloned()
                .unwrap_or_else(|| "0.0".to_string()),
        })
        .collect();
}

fn parse_named_value_rows(
    label: &str,
    rows: &[SimulationNamedValueRow],
    require_non_negative: bool,
) -> Result<HashMap<String, f64>, String> {
    let mut values = HashMap::new();
    for row in rows {
        let parsed = row
            .value
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("{} '{}' ist keine gültige Zahl", label, row.name))?;
        if !parsed.is_finite() {
            return Err(format!("{} '{}' muss endlich sein", label, row.name));
        }
        if require_non_negative && parsed < 0.0 {
            return Err(format!("{} '{}' muss >= 0 sein", label, row.name));
        }
        values.insert(row.name.clone(), parsed);
    }
    Ok(values)
}

fn parse_live_simulation_scalar(
    field: &SimulationLiveScalarField,
    raw: &str,
) -> Result<f64, String> {
    let parsed = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| "draft value is not yet a valid number".to_string())?;
    if !parsed.is_finite() {
        return Err("draft value must be finite".to_string());
    }
    match field {
        SimulationLiveScalarField::CvNoiseStd(_) | SimulationLiveScalarField::MvNoiseStd(_)
            if parsed < 0.0 =>
        {
            Err("noise std must be >= 0".to_string())
        }
        _ => Ok(parsed),
    }
}

fn parse_live_simulation_matrix_value(raw: &str) -> Result<f64, String> {
    let parsed = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| "draft value is not yet a valid number".to_string())?;
    if !parsed.is_finite() {
        return Err("draft value must be finite".to_string());
    }
    Ok(parsed)
}

fn parse_matrix_import_text(
    raw: &str,
    expected_rows: usize,
    expected_cols: usize,
) -> Result<Vec<Vec<String>>, String> {
    let trimmed = raw.trim();

    if expected_rows == 0 {
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        return Err("matrix has no rows in the current shape".to_string());
    }

    if expected_cols == 0 {
        if trimmed.is_empty() {
            return Ok(vec![Vec::new(); expected_rows]);
        }
        return Err("matrix has no columns in the current shape".to_string());
    }

    if trimmed.is_empty() {
        return Err("import text is empty".to_string());
    }

    let parsed_rows: Vec<Vec<String>> = trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(split_import_row)
        .collect::<Result<_, _>>()?;

    if parsed_rows.len() != expected_rows {
        return Err(format!(
            "expected {} rows but received {}",
            expected_rows,
            parsed_rows.len()
        ));
    }

    for (row_index, row) in parsed_rows.iter().enumerate() {
        if row.len() != expected_cols {
            return Err(format!(
                "row {} has {} columns, expected {}",
                row_index + 1,
                row.len(),
                expected_cols
            ));
        }
    }

    Ok(parsed_rows)
}

fn split_import_row(line: &str) -> Result<Vec<String>, String> {
    let splitter = if line.contains('\t') {
        '\t'
    } else if line.contains(';') {
        ';'
    } else if line.contains(',') {
        ','
    } else {
        return Ok(vec![line.trim().to_string()]);
    };

    let cells: Vec<String> = line
        .split(splitter)
        .map(|cell| cell.trim().to_string())
        .collect();

    if cells.iter().any(|cell| cell.is_empty()) {
        return Err("import contains an empty cell".to_string());
    }

    Ok(cells)
}

#[cfg(test)]
mod tests {
    use crate::project_config::{
        CvIoDefinition, LogicParamValue, MvIoDefinition, ProjectConfig, VariableCatalogConfig,
        WatchdogConfig,
    };

    use super::{
        ModelEditorState, ModelKindChoice, build_model_config_from_editor,
        parse_matrix_import_text, resize_string_matrix, sync_io_initial_value_online_params,
    };

    fn model_test_vars() -> VariableCatalogConfig {
        VariableCatalogConfig {
            mvs: vec!["u1".to_string(), "u2".to_string()],
            cvs: vec!["y1".to_string(), "y2".to_string()],
            dvs: vec!["d1".to_string()],
        }
    }

    #[test]
    fn syncs_static_io_initial_values_into_online_params() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.watchdog = WatchdogConfig::None;
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: "ns=2;s=u1_write".to_string(),
            mv_read: "ns=2;s=u1_read".to_string(),
            hl_opc: None,
            ll_opc: None,
            hl: Some(10.0),
            ll: Some(-10.0),
            enable: None,
        });
        cfg.io_definition.cvs.push(CvIoDefinition {
            name: "y1".to_string(),
            soll: "ns=2;s=y1_target".to_string(),
            ist: "ns=2;s=y1_value".to_string(),
            sollband_opc: None,
            sollband_value: Some(0.5),
            enable: None,
        });

        sync_io_initial_value_online_params(&mut cfg);

        assert_eq!(
            cfg.logic.params.get("io.mv.u1.hl"),
            Some(&LogicParamValue::F64(10.0))
        );
        assert_eq!(
            cfg.logic.params.get("io.mv.u1.ll"),
            Some(&LogicParamValue::F64(-10.0))
        );
        assert_eq!(
            cfg.logic.params.get("io.cv.y1.sollband"),
            Some(&LogicParamValue::F64(0.5))
        );
    }

    #[test]
    fn removes_static_online_param_when_opc_address_exists() {
        let mut cfg = ProjectConfig::new("demo");
        cfg.logic
            .params
            .insert("io.mv.u1.hl".to_string(), LogicParamValue::F64(10.0));
        cfg.io_definition.mvs.push(MvIoDefinition {
            name: "u1".to_string(),
            mv_write: "ns=2;s=u1_write".to_string(),
            mv_read: "ns=2;s=u1_read".to_string(),
            hl_opc: Some("ns=2;s=u1_hl".to_string()),
            ll_opc: None,
            hl: Some(10.0),
            ll: Some(-10.0),
            enable: None,
        });

        sync_io_initial_value_online_params(&mut cfg);

        assert!(!cfg.logic.params.contains_key("io.mv.u1.hl"));
        assert_eq!(
            cfg.logic.params.get("io.mv.u1.ll"),
            Some(&LogicParamValue::F64(-10.0))
        );
    }

    #[test]
    fn parses_excel_tsv_matrix_import() {
        let parsed =
            parse_matrix_import_text("1\t2\t3\n4\t5\t6", 2, 3).expect("tsv import should parse");

        assert_eq!(
            parsed,
            vec![
                vec!["1".to_string(), "2".to_string(), "3".to_string()],
                vec!["4".to_string(), "5".to_string(), "6".to_string()],
            ]
        );
    }

    #[test]
    fn parses_comma_and_semicolon_matrix_import() {
        let comma = parse_matrix_import_text("1,2\n3,4", 2, 2).expect("comma import should parse");
        let semicolon =
            parse_matrix_import_text("1;2\n3;4", 2, 2).expect("semicolon import should parse");

        assert_eq!(
            comma,
            vec![
                vec!["1".to_string(), "2".to_string()],
                vec!["3".to_string(), "4".to_string()],
            ]
        );
        assert_eq!(comma, semicolon);
    }

    #[test]
    fn resize_string_matrix_preserves_top_left_overlap() {
        let resized = resize_string_matrix(
            vec![
                vec!["1".to_string(), "2".to_string()],
                vec!["3".to_string(), "4".to_string()],
            ],
            3,
            4,
        );

        assert_eq!(resized[0][0], "1");
        assert_eq!(resized[0][1], "2");
        assert_eq!(resized[1][0], "3");
        assert_eq!(resized[1][1], "4");
        assert_eq!(resized[2][3], "0.0");
    }

    #[test]
    fn save_model_reports_cell_level_numeric_errors() {
        let vars = model_test_vars();
        let mut editor = ModelEditorState::blank(&vars);
        editor.selected_kind = ModelKindChoice::LinearStateSpace;
        editor.state_count = "2".to_string();
        editor.resize_matrices_for(&vars);
        editor.a_cells[0][1] = "oops".to_string();

        let err = build_model_config_from_editor(&editor, &vars)
            .expect_err("invalid numeric cell should fail");

        assert!(err.contains("A[1, 2]"));
    }
}
