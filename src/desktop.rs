use std::{
    collections::{BTreeMap, HashMap},
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
    project::ProjectService,
    project_config::SidebarTask,
    runtime::{RuntimeManager, RuntimeSnapshot},
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
        size: iced::Size::new(1320.0, 860.0),
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
enum WebStatus {
    Disabled,
    Starting(String),
    Running(String),
    Error(String),
}

#[derive(Debug, Clone)]
struct ProjectPanelData {
    sidebar_tasks: Vec<SidebarTask>,
    config_params: HashMap<String, f64>,
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
    new_project_name: String,
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
            new_project_name: String::new(),
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
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let header = self.header_view();
        let runtime = self.runtime_view();
        let sidebar = self.sidebar_view();
        let params = self.params_view();
        let plots = self.plots_view();
        let status = self.status_view();

        let main = column![
            header,
            runtime,
            row![
                sidebar,
                container(column![params, plots].spacing(16)).width(Length::Fill)
            ]
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
                content = content.push(text(format!("{} {}", marker, task.label)));
            }
        }

        container(scrollable(content))
            .padding(12)
            .width(Length::FillPortion(1))
            .height(Length::Fill)
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

        container(scrollable(content).height(Length::Fixed(250.0)))
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
            Ok(ProjectPanelData {
                sidebar_tasks,
                config_params: cfg.logic.params,
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
