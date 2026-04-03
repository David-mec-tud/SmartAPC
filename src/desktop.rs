use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::anyhow;
use iced::{
    Element, Length, Subscription, Task, Theme, application, time,
    widget::{button, column, container, pick_list, row, scrollable, text, text_input},
};

use crate::{
    communication::{ProtocolAdapter, opcua::OpcUaClientAdapter},
    project::ProjectService,
    project_config::{NumericLimit, SidebarTask, SignalPoint, WatchdogConfig},
    runtime::{RuntimeManager, RuntimeSnapshot},
    storage::SignalSample,
    types::{SignalType, SignalValue},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalTypeChoice {
    F64,
    Bool,
}

const SIGNAL_TYPE_OPTIONS: [SignalTypeChoice; 2] = [SignalTypeChoice::F64, SignalTypeChoice::Bool];

impl fmt::Display for SignalTypeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalTypeChoice::F64 => write!(f, "f64"),
            SignalTypeChoice::Bool => write!(f, "bool"),
        }
    }
}

impl From<SignalType> for SignalTypeChoice {
    fn from(value: SignalType) -> Self {
        match value {
            SignalType::F64 => Self::F64,
            SignalType::Bool => Self::Bool,
        }
    }
}

impl From<SignalTypeChoice> for SignalType {
    fn from(value: SignalTypeChoice) -> Self {
        match value {
            SignalTypeChoice::F64 => Self::F64,
            SignalTypeChoice::Bool => Self::Bool,
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
struct SignalPointEditorRow {
    name: String,
    signal_type: SignalTypeChoice,
    address: String,
    opc_tag: String,
}

impl SignalPointEditorRow {
    fn blank() -> Self {
        Self {
            name: String::new(),
            signal_type: SignalTypeChoice::F64,
            address: String::new(),
            opc_tag: String::new(),
        }
    }

    fn from_signal_point(point: &SignalPoint) -> Self {
        Self {
            name: point.name.clone(),
            signal_type: point.signal_type.into(),
            address: point.address.clone(),
            opc_tag: point.opc_tag.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone)]
struct NumericLimitEditorRow {
    signal: String,
    min: String,
    max: String,
    opc_tag: String,
}

impl NumericLimitEditorRow {
    fn blank() -> Self {
        Self {
            signal: String::new(),
            min: String::new(),
            max: String::new(),
            opc_tag: String::new(),
        }
    }

    fn from_limit(signal: &str, limit: &NumericLimit) -> Self {
        Self {
            signal: signal.to_string(),
            min: format!("{}", limit.min),
            max: format!("{}", limit.max),
            opc_tag: limit.opc_tag.clone().unwrap_or_default(),
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

#[derive(Debug, Clone)]
enum WebStatus {
    Disabled,
    Starting(String),
    Running(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupPage {
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
            "watchdog" => Some(Self::Watchdog),
            "io" => Some(Self::Io),
            "limits" => Some(Self::Limits),
            "model" => Some(Self::Model),
            "logic" => Some(Self::Logic),
            "timing" => Some(Self::Timing),
            "simulation" => Some(Self::Simulation),
            "deployment" => Some(Self::Deployment),
            "params" => Some(Self::Params),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Watchdog => "Watchdog",
            Self::Io => "Ein- und Ausgänge",
            Self::Limits => "Limits",
            Self::Model => "Modell",
            Self::Logic => "Regellogik",
            Self::Timing => "Ausführungszeittest",
            Self::Simulation => "Reglersimulation",
            Self::Deployment => "Deployment",
            Self::Params => "Online Parameter",
            Self::Plots => "Plots",
        }
    }
}

#[derive(Debug, Clone)]
struct ProjectPanelData {
    sidebar_tasks: Vec<SidebarTask>,
    config_params: HashMap<String, f64>,
    io_inputs: Vec<SignalPointEditorRow>,
    io_outputs: Vec<SignalPointEditorRow>,
    limits: Vec<NumericLimitEditorRow>,
    watchdog: WatchdogEditorState,
}

#[derive(Debug, Clone)]
struct RefreshData {
    snapshot: Option<RuntimeSnapshot>,
    params: Option<HashMap<String, f64>>,
    samples: Vec<SignalSample>,
}

#[derive(Debug, Clone)]
struct ParamApplyResult {
    params: HashMap<String, f64>,
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
    WatchdogKindChanged(WatchdogKindChoice),
    WatchdogReadTagChanged(String),
    WatchdogWriteTagChanged(String),
    WatchdogCycleChanged(String),
    WatchdogTimeoutChanged(String),
    SaveWatchdogPressed,
    WatchdogSaved(Result<String, String>),
    TestWatchdogPressed,
    WatchdogTested(Result<String, String>),
    AddInputRow,
    RemoveInputRow(usize),
    InputRowNameChanged(usize, String),
    InputRowTypeChanged(usize, SignalTypeChoice),
    InputRowAddressChanged(usize, String),
    InputRowOpcTagChanged(usize, String),
    AddOutputRow,
    RemoveOutputRow(usize),
    OutputRowNameChanged(usize, String),
    OutputRowTypeChanged(usize, SignalTypeChoice),
    OutputRowAddressChanged(usize, String),
    OutputRowOpcTagChanged(usize, String),
    SaveIoPressed,
    IoSaved(Result<String, String>),
    LimitMinChanged(String, String),
    LimitMaxChanged(String, String),
    LimitOpcTagChanged(String, String),
    SaveLimitsPressed,
    LimitsSaved(Result<String, String>),
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
    logic_params: HashMap<String, f64>,
    param_inputs: HashMap<String, String>,
    samples: Vec<SignalSample>,
    runtime_snapshot: Option<RuntimeSnapshot>,
    selected_page: SetupPage,
    new_project_name: String,
    io_input_rows: Vec<SignalPointEditorRow>,
    io_output_rows: Vec<SignalPointEditorRow>,
    limit_rows: Vec<NumericLimitEditorRow>,
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
            samples: Vec::new(),
            runtime_snapshot: None,
            selected_page: SetupPage::Watchdog,
            new_project_name: String::new(),
            io_input_rows: vec![SignalPointEditorRow::blank()],
            io_output_rows: vec![SignalPointEditorRow::blank()],
            limit_rows: vec![NumericLimitEditorRow::blank()],
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
                    self.io_input_rows = panel.io_inputs;
                    self.io_output_rows = panel.io_outputs;
                    self.limit_rows = panel.limits;
                    self.watchdog_editor = panel.watchdog;

                    if self.runtime_snapshot.is_none() {
                        self.logic_params = panel.config_params;
                        self.refresh_param_inputs();
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
                    self.runtime_snapshot = data.snapshot;
                    self.samples = data.samples;
                    if let Some(params) = data.params {
                        self.logic_params = params;
                        self.refresh_param_inputs();
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
                self.param_inputs.insert(key, value);
                Task::none()
            }
            Message::ApplyParamPressed(key) => {
                let Some(raw_value) = self.param_inputs.get(&key).cloned() else {
                    self.error_message = Some(format!("Kein Eingabewert für Parameter '{}'", key));
                    return Task::none();
                };
                let parsed = match raw_value.parse::<f64>() {
                    Ok(value) => value,
                    Err(_) => {
                        self.error_message = Some(format!(
                            "Ungültiger Zahlenwert '{}' für Parameter '{}'",
                            raw_value, key
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
                    self.logic_params = updated.params;
                    self.refresh_param_inputs();
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
                if self.runtime_snapshot.is_some() {
                    self.error_message = Some(
                        "Runtime läuft. Für strukturelle Konfig-Änderungen bitte erst stoppen."
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
                if self.runtime_snapshot.is_some() {
                    self.error_message = Some(
                        "Runtime läuft. Für den Watchdog-Test bitte erst stoppen.".to_string(),
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
            Message::AddInputRow => {
                self.io_input_rows.push(SignalPointEditorRow::blank());
                Task::none()
            }
            Message::RemoveInputRow(index) => {
                if self.io_input_rows.len() > 1 && index < self.io_input_rows.len() {
                    self.io_input_rows.remove(index);
                }
                Task::none()
            }
            Message::InputRowNameChanged(index, value) => {
                if let Some(row) = self.io_input_rows.get_mut(index) {
                    row.name = value;
                }
                Task::none()
            }
            Message::InputRowTypeChanged(index, signal_type) => {
                if let Some(row) = self.io_input_rows.get_mut(index) {
                    row.signal_type = signal_type;
                }
                Task::none()
            }
            Message::InputRowAddressChanged(index, value) => {
                if let Some(row) = self.io_input_rows.get_mut(index) {
                    row.address = value;
                }
                Task::none()
            }
            Message::InputRowOpcTagChanged(index, value) => {
                if let Some(row) = self.io_input_rows.get_mut(index) {
                    row.opc_tag = value;
                }
                Task::none()
            }
            Message::AddOutputRow => {
                self.io_output_rows.push(SignalPointEditorRow::blank());
                Task::none()
            }
            Message::RemoveOutputRow(index) => {
                if self.io_output_rows.len() > 1 && index < self.io_output_rows.len() {
                    self.io_output_rows.remove(index);
                }
                Task::none()
            }
            Message::OutputRowNameChanged(index, value) => {
                if let Some(row) = self.io_output_rows.get_mut(index) {
                    row.name = value;
                }
                Task::none()
            }
            Message::OutputRowTypeChanged(index, signal_type) => {
                if let Some(row) = self.io_output_rows.get_mut(index) {
                    row.signal_type = signal_type;
                }
                Task::none()
            }
            Message::OutputRowAddressChanged(index, value) => {
                if let Some(row) = self.io_output_rows.get_mut(index) {
                    row.address = value;
                }
                Task::none()
            }
            Message::OutputRowOpcTagChanged(index, value) => {
                if let Some(row) = self.io_output_rows.get_mut(index) {
                    row.opc_tag = value;
                }
                Task::none()
            }
            Message::SaveIoPressed => {
                if self.runtime_snapshot.is_some() {
                    self.error_message = Some(
                        "Runtime läuft. Für strukturelle Konfig-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_io_task(
                    self.project_service.clone(),
                    project_name,
                    self.io_input_rows.clone(),
                    self.io_output_rows.clone(),
                )
            }
            Message::IoSaved(result) => match result {
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
            Message::LimitMinChanged(signal, value) => {
                self.ensure_limit_row_exists(&signal);
                if let Some(row) = self.find_limit_row_mut(&signal) {
                    row.min = value;
                }
                Task::none()
            }
            Message::LimitMaxChanged(signal, value) => {
                self.ensure_limit_row_exists(&signal);
                if let Some(row) = self.find_limit_row_mut(&signal) {
                    row.max = value;
                }
                Task::none()
            }
            Message::LimitOpcTagChanged(signal, value) => {
                self.ensure_limit_row_exists(&signal);
                if let Some(row) = self.find_limit_row_mut(&signal) {
                    row.opc_tag = value;
                }
                Task::none()
            }
            Message::SaveLimitsPressed => {
                if self.runtime_snapshot.is_some() {
                    self.error_message = Some(
                        "Runtime läuft. Für strukturelle Konfig-Änderungen bitte erst stoppen."
                            .to_string(),
                    );
                    return Task::none();
                }

                let Some(project_name) = self.selected_project.clone() else {
                    self.error_message = Some("Bitte zuerst ein Projekt auswählen".to_string());
                    return Task::none();
                };

                save_limits_task(
                    self.project_service.clone(),
                    project_name,
                    self.numeric_signal_names(),
                    self.limit_rows.clone(),
                )
            }
            Message::LimitsSaved(result) => match result {
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
        let runtime_running = self.runtime_snapshot.is_some();
        let start_button = if self.selected_project.is_some() && !runtime_running {
            button("Runtime Start").on_press(Message::StartRuntimePressed)
        } else {
            button("Runtime Start")
        };

        let stop_button = if runtime_running {
            button("Runtime Stop").on_press(Message::StopRuntimePressed)
        } else {
            button("Runtime Stop")
        };

        let summary = if let Some(snapshot) = &self.runtime_snapshot {
            format!(
                "Projekt={} | State={:?} | Watchdog={:?} | Cycles={} | DeadlineMiss={} | LastDuration={}us | LastJitter={}us",
                snapshot.project_name,
                snapshot.state,
                snapshot.watchdog_status,
                snapshot.metrics.total_cycles,
                snapshot.metrics.deadline_misses,
                snapshot.metrics.last_duration_us,
                snapshot.metrics.last_jitter_us
            )
        } else {
            "Runtime nicht aktiv".to_string()
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
        let runtime_running = self.runtime_snapshot.is_some();
        match self.selected_page {
            SetupPage::Watchdog => self.watchdog_editor_view(runtime_running),
            SetupPage::Io => self.io_editor_view(runtime_running),
            SetupPage::Limits => self.limits_editor_view(runtime_running),
            SetupPage::Model => self.placeholder_page_view(
                SetupPage::Model,
                "V1: statisch kompilierte Modulanbindung. Im nächsten Schritt ergänzen wir hier Modell-Auswahl + Model-Config-Felder.",
            ),
            SetupPage::Logic => self.placeholder_page_view(
                SetupPage::Logic,
                "V1: Mapping-Logik ist aktiv. Als nächstes können wir hier Logik-Typ-Auswahl und dazugehörige Konfigurationsfelder ergänzen.",
            ),
            SetupPage::Timing => self.placeholder_page_view(
                SetupPage::Timing,
                "Hier folgt ein Wizard für Zykluszeit-/Jitter-Checks mit sichtbaren Metriken.",
            ),
            SetupPage::Simulation => self.placeholder_page_view(
                SetupPage::Simulation,
                "Hier folgt die Simulationsoberfläche mit Eingangsprofilen und Ergebnisplots.",
            ),
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
                    "Runtime läuft: Speichern deaktiviert"
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

        let mut inputs_col = column![
            row![
                text("Inputs").size(18),
                button("+ Zeile").on_press(Message::AddInputRow)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(6);

        for (index, entry) in self.io_input_rows.iter().enumerate() {
            inputs_col = inputs_col.push(
                row![
                    text_input("Name", &entry.name)
                        .on_input(move |value| Message::InputRowNameChanged(index, value))
                        .width(Length::FillPortion(2)),
                    pick_list(
                        SIGNAL_TYPE_OPTIONS.to_vec(),
                        Some(entry.signal_type),
                        move |value| { Message::InputRowTypeChanged(index, value) }
                    )
                    .width(Length::Fixed(90.0)),
                    text_input("OPC-UA Adresse", &entry.address)
                        .on_input(move |value| Message::InputRowAddressChanged(index, value))
                        .width(Length::FillPortion(3)),
                    text_input("OPC Tag (optional)", &entry.opc_tag)
                        .on_input(move |value| Message::InputRowOpcTagChanged(index, value))
                        .width(Length::FillPortion(2)),
                    button("-").on_press(Message::RemoveInputRow(index))
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        let mut outputs_col = column![
            row![
                text("Outputs").size(18),
                button("+ Zeile").on_press(Message::AddOutputRow)
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        ]
        .spacing(6);

        for (index, entry) in self.io_output_rows.iter().enumerate() {
            outputs_col = outputs_col.push(
                row![
                    text_input("Name", &entry.name)
                        .on_input(move |value| Message::OutputRowNameChanged(index, value))
                        .width(Length::FillPortion(2)),
                    pick_list(
                        SIGNAL_TYPE_OPTIONS.to_vec(),
                        Some(entry.signal_type),
                        move |value| { Message::OutputRowTypeChanged(index, value) }
                    )
                    .width(Length::Fixed(90.0)),
                    text_input("OPC-UA Adresse", &entry.address)
                        .on_input(move |value| Message::OutputRowAddressChanged(index, value))
                        .width(Length::FillPortion(3)),
                    text_input("OPC Tag (optional)", &entry.opc_tag)
                        .on_input(move |value| Message::OutputRowOpcTagChanged(index, value))
                        .width(Length::FillPortion(2)),
                    button("-").on_press(Message::RemoveOutputRow(index))
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        let save_button = if can_save {
            button("I/O speichern").on_press(Message::SaveIoPressed)
        } else {
            button("I/O speichern")
        };

        let hint = if runtime_running {
            text("Runtime läuft: Für I/O-Änderungen erst stoppen.").size(13)
        } else {
            text("").size(13)
        };

        container(
            column![
                text("I/O Definition").size(20),
                inputs_col,
                outputs_col,
                hint,
                save_button
            ]
            .spacing(10),
        )
        .padding(10)
        .into()
    }

    fn limits_editor_view(&self, runtime_running: bool) -> Element<'_, Message> {
        let can_save = self.selected_project.is_some() && !runtime_running;
        let signal_names = self.numeric_signal_names();

        let mut content = column![text("Limits").size(20)].spacing(10);

        if signal_names.is_empty() {
            content = content.push(text(
                "Keine numerischen I/O-Signale definiert. Bitte zuerst I/O konfigurieren.",
            ));
        } else {
            for signal in signal_names {
                let existing = self.find_limit_row(&signal);
                let min_value = existing.map(|row| row.min.clone()).unwrap_or_default();
                let max_value = existing.map(|row| row.max.clone()).unwrap_or_default();
                let opc_tag_value = existing.map(|row| row.opc_tag.clone()).unwrap_or_default();

                let signal_for_min = signal.clone();
                let signal_for_max = signal.clone();
                let signal_for_tag = signal.clone();

                content = content.push(
                    row![
                        text(signal).width(Length::FillPortion(2)),
                        text_input("Min", &min_value)
                            .on_input(move |value| {
                                Message::LimitMinChanged(signal_for_min.clone(), value)
                            })
                            .width(Length::FillPortion(1)),
                        text_input("Max", &max_value)
                            .on_input(move |value| {
                                Message::LimitMaxChanged(signal_for_max.clone(), value)
                            })
                            .width(Length::FillPortion(1)),
                        text_input("OPC Tag (optional)", &opc_tag_value)
                            .on_input(move |value| {
                                Message::LimitOpcTagChanged(signal_for_tag.clone(), value)
                            })
                            .width(Length::FillPortion(2)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                );
            }
        }

        let save_button = if can_save {
            button("Limits speichern").on_press(Message::SaveLimitsPressed)
        } else {
            button("Limits speichern")
        };

        let hint = if runtime_running {
            text("Runtime läuft: Für Limits-Änderungen erst stoppen.").size(13)
        } else {
            text("").size(13)
        };

        container(column![content, hint, save_button].spacing(10))
            .padding(10)
            .into()
    }

    fn params_view(&self) -> Element<'_, Message> {
        let runtime_running = self.runtime_snapshot.is_some();
        let mut content = column![text("Online Parameter").size(20)].spacing(10);

        if self.logic_params.is_empty() {
            content = content.push(text("Keine Parameter vorhanden."));
        } else {
            let mut keys: Vec<_> = self.logic_params.keys().cloned().collect();
            keys.sort();

            for key in keys {
                let current = self.logic_params.get(&key).copied().unwrap_or_default();
                let input_value = self
                    .param_inputs
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| format!("{current:.4}"));
                let key_clone = key.clone();

                let apply_button = if runtime_running {
                    button("Apply").on_press(Message::ApplyParamPressed(key_clone))
                } else {
                    button("Apply")
                };

                content = content.push(
                    row![
                        text(key.clone()).width(Length::FillPortion(2)),
                        text(format!("{current:.4}")).width(Length::FillPortion(1)),
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
        let mut content = column![text("Plots (SQLite)").size(20)].spacing(8);
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

    fn status_view(&self) -> Element<'_, Message> {
        let mut line = self.status_message.clone();
        if let Some(err) = &self.error_message {
            line = format!("{} | Fehler: {}", line, err);
        }
        container(text(line).size(14)).padding(8).into()
    }

    fn refresh_param_inputs(&mut self) {
        let mut next = HashMap::new();
        for (key, value) in &self.logic_params {
            next.insert(key.clone(), format!("{value:.4}"));
        }
        self.param_inputs = next;
    }

    fn numeric_signal_names(&self) -> Vec<String> {
        let mut names = BTreeSet::new();

        for row in self.io_input_rows.iter().chain(self.io_output_rows.iter()) {
            if row.signal_type == SignalTypeChoice::F64 {
                let name = row.name.trim();
                if !name.is_empty() {
                    names.insert(name.to_string());
                }
            }
        }

        names.into_iter().collect()
    }

    fn find_limit_row(&self, signal: &str) -> Option<&NumericLimitEditorRow> {
        self.limit_rows.iter().find(|row| row.signal == signal)
    }

    fn find_limit_row_mut(&mut self, signal: &str) -> Option<&mut NumericLimitEditorRow> {
        self.limit_rows.iter_mut().find(|row| row.signal == signal)
    }

    fn ensure_limit_row_exists(&mut self, signal: &str) {
        if self.find_limit_row(signal).is_some() {
            return;
        }
        self.limit_rows.push(NumericLimitEditorRow {
            signal: signal.to_string(),
            min: String::new(),
            max: String::new(),
            opc_tag: String::new(),
        });
    }
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

            let io_inputs = if cfg.io.inputs.is_empty() {
                vec![SignalPointEditorRow::blank()]
            } else {
                cfg.io
                    .inputs
                    .iter()
                    .map(SignalPointEditorRow::from_signal_point)
                    .collect()
            };

            let io_outputs = if cfg.io.outputs.is_empty() {
                vec![SignalPointEditorRow::blank()]
            } else {
                cfg.io
                    .outputs
                    .iter()
                    .map(SignalPointEditorRow::from_signal_point)
                    .collect()
            };

            let mut limit_rows: Vec<_> = cfg
                .limits
                .values
                .iter()
                .map(|(signal, limit)| NumericLimitEditorRow::from_limit(signal, limit))
                .collect();
            limit_rows.sort_by(|a, b| a.signal.cmp(&b.signal));
            if limit_rows.is_empty() {
                limit_rows.push(NumericLimitEditorRow::blank());
            }

            Ok(ProjectPanelData {
                sidebar_tasks,
                config_params: cfg.logic.params,
                io_inputs,
                io_outputs,
                limits: limit_rows,
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

fn refresh_task(runtime: Arc<RuntimeManager>) -> Task<Message> {
    Task::perform(
        async move {
            let snapshot = runtime
                .state()
                .await
                .map_err(|err| format!("Runtime-Status konnte nicht gelesen werden: {err}"))?;

            if snapshot.is_none() {
                return Ok(RefreshData {
                    snapshot: None,
                    params: None,
                    samples: Vec::new(),
                });
            }

            let params = runtime
                .get_params()
                .await
                .map_err(|err| format!("Runtime-Parameter konnten nicht gelesen werden: {err}"))?;
            let samples = runtime
                .read_plots(400)
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
    value: f64,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut updates = HashMap::new();
            updates.insert(key, value);

            let params = runtime
                .set_params(updates.clone())
                .await
                .map_err(|err| format!("Runtime-Parameter konnten nicht gesetzt werden: {err}"))?;

            if let Some(project_name) = runtime.active_project().await {
                project_service
                    .update_logic_params(&project_name, updates)
                    .map_err(|err| format!("Parameterpersistenz fehlgeschlagen: {err}"))?;
            }

            Ok(ParamApplyResult { params })
        },
        Message::ParamApplied,
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

fn save_io_task(
    project_service: ProjectService,
    project_name: String,
    input_rows: Vec<SignalPointEditorRow>,
    output_rows: Vec<SignalPointEditorRow>,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            cfg.io.inputs = build_signal_points("input", &input_rows)?;
            cfg.io.outputs = build_signal_points("output", &output_rows)?;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("I/O konnten nicht gespeichert werden: {err}"))?;

            Ok(format!("I/O für '{}' gespeichert", project_name))
        },
        Message::IoSaved,
    )
}

fn save_limits_task(
    project_service: ProjectService,
    project_name: String,
    signals: Vec<String>,
    limit_rows: Vec<NumericLimitEditorRow>,
) -> Task<Message> {
    Task::perform(
        async move {
            let mut cfg = project_service.load_config(&project_name).map_err(|err| {
                format!("Projektkonfiguration konnte nicht geladen werden: {err}")
            })?;

            cfg.limits.values = build_limits_map_for_signals(&signals, &limit_rows)?;

            project_service
                .save_config(&project_name, &cfg)
                .map_err(|err| format!("Limits konnten nicht gespeichert werden: {err}"))?;

            Ok(format!("Limits für '{}' gespeichert", project_name))
        },
        Message::LimitsSaved,
    )
}

fn build_signal_points(
    kind: &str,
    rows: &[SignalPointEditorRow],
) -> Result<Vec<SignalPoint>, String> {
    let mut out = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let blank = row.name.trim().is_empty()
            && row.address.trim().is_empty()
            && row.opc_tag.trim().is_empty();

        if blank {
            continue;
        }

        if row.name.trim().is_empty() {
            return Err(format!(
                "{}-Zeile {}: Name darf nicht leer sein",
                kind,
                index + 1
            ));
        }

        if row.address.trim().is_empty() {
            return Err(format!(
                "{}-Zeile {}: OPC-UA Adresse darf nicht leer sein",
                kind,
                index + 1
            ));
        }

        out.push(SignalPoint {
            name: row.name.trim().to_string(),
            signal_type: row.signal_type.into(),
            address: row.address.trim().to_string(),
            opc_tag: optional_trimmed(&row.opc_tag),
        });
    }

    Ok(out)
}

fn build_limits_map_for_signals(
    signals: &[String],
    rows: &[NumericLimitEditorRow],
) -> Result<HashMap<String, NumericLimit>, String> {
    let mut out = HashMap::new();

    for signal in signals {
        let row = rows.iter().find(|row| row.signal == *signal);

        let (min_raw, max_raw, tag_raw) = if let Some(row) = row {
            (row.min.trim(), row.max.trim(), row.opc_tag.trim())
        } else {
            ("", "", "")
        };

        let blank = min_raw.is_empty() && max_raw.is_empty() && tag_raw.is_empty();
        if blank {
            continue;
        }

        if min_raw.is_empty() || max_raw.is_empty() {
            return Err(format!(
                "Limit für Signal '{}' benötigt Min und Max oder beide leer",
                signal
            ));
        }

        let min = min_raw.parse::<f64>().map_err(|_| {
            format!(
                "Limit '{}' Min '{}' ist keine gültige Zahl",
                signal, min_raw
            )
        })?;
        let max = max_raw.parse::<f64>().map_err(|_| {
            format!(
                "Limit '{}' Max '{}' ist keine gültige Zahl",
                signal, max_raw
            )
        })?;

        out.insert(
            signal.clone(),
            NumericLimit {
                min,
                max,
                opc_tag: optional_trimmed(tag_raw),
            },
        );
    }

    Ok(out)
}

fn optional_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
