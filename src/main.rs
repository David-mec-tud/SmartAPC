use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::{Parser, Subcommand};

#[cfg(feature = "legacy-native")]
use smartapc::{
    desktop::{DesktopSettings, run as run_desktop},
    virtual_hardware::run_native,
};
use smartapc::{
    project::ProjectService,
    runtime::RuntimeManager,
    virtual_hardware::{VirtualHardwareConfig, run as run_virtual_hardware, run_browser},
    web::{AppState, serve},
};

#[derive(Debug, Parser)]
#[command(author, version, about = "SmartAPC V1 runtime")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[cfg(feature = "legacy-native")]
    Desktop {
        #[arg(long, default_value = "./projects")]
        project_root: PathBuf,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value_t = false)]
        no_web: bool,
    },
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value = "./projects")]
        project_root: PathBuf,
        #[arg(long)]
        project: Option<String>,
    },
    InitProject {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "./projects")]
        project_root: PathBuf,
    },
    VirtualHardware {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4855)]
        port: u16,
        #[arg(long, default_value_t = 100)]
        dt_ms: u64,
        #[arg(long, default_value_t = 5.0)]
        tau_s: f64,
        #[arg(long, default_value_t = 1.0)]
        gain: f64,
        #[arg(long, default_value_t = 120)]
        history: usize,
        /// Serve the local React/Plotly feedback page on this HTTP port.
        #[arg(long, default_value_t = 8081)]
        web_port: u16,
        /// Run only the OPC UA plant, with no HTTP feedback server.
        #[arg(long)]
        no_web: bool,
        /// Use the transitional native plot window instead of browser feedback.
        #[arg(long, conflicts_with = "no_web")]
        #[cfg(feature = "legacy-native")]
        native_plot: bool,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Serve {
        host: "127.0.0.1".to_string(),
        port: 8080,
        project_root: PathBuf::from("./projects"),
        project: None,
    }) {
        #[cfg(feature = "legacy-native")]
        Command::Desktop {
            project_root,
            host,
            port,
            no_web,
        } => {
            std::fs::create_dir_all(&project_root).with_context(|| {
                format!("failed to create project root {}", project_root.display())
            })?;

            let web_bind = if no_web {
                None
            } else {
                Some(
                    format!("{host}:{port}")
                        .parse()
                        .context("invalid desktop web host/port")?,
                )
            };

            run_desktop(DesktopSettings {
                project_root,
                executable_path: std::env::current_exe()
                    .context("unable to resolve current exe")?,
                web_bind,
            })
        }
        Command::Serve {
            host,
            port,
            project_root,
            project,
        } => run_serve(host, port, project_root, project),
        Command::InitProject { name, project_root } => {
            std::fs::create_dir_all(&project_root).with_context(|| {
                format!("failed to create project root {}", project_root.display())
            })?;

            let service = ProjectService::new(&project_root);
            let cfg = service.create_project(&name)?;
            println!(
                "created project '{}' at {}",
                cfg.name,
                service.project_dir(&name).display()
            );
            Ok(())
        }
        Command::VirtualHardware {
            host,
            port,
            dt_ms,
            tau_s,
            gain,
            history,
            web_port,
            no_web,
            #[cfg(feature = "legacy-native")]
            native_plot,
        } => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to create tokio runtime for virtual hardware")?;

            let web_bind: SocketAddr = format!("{host}:{web_port}")
                .parse()
                .context("invalid virtual hardware web host/port")?;
            let config = VirtualHardwareConfig {
                host,
                port,
                dt_ms,
                tau_s,
                gain,
                history,
            };
            runtime.block_on(async {
                #[cfg(feature = "legacy-native")]
                if native_plot {
                    return run_native(config).await;
                }
                if no_web {
                    run_virtual_hardware(config).await
                } else {
                    run_browser(config, web_bind).await
                }
            })
        }
    }
}

fn run_serve(
    host: String,
    port: u16,
    project_root: PathBuf,
    project: Option<String>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime for web server")?;

    runtime.block_on(async move {
        std::fs::create_dir_all(&project_root)
            .with_context(|| format!("failed to create project root {}", project_root.display()))?;

        let project_service = ProjectService::new(&project_root);
        let runtime = Arc::new(RuntimeManager::new());

        if let Some(project_name) = project {
            runtime
                .start(&project_service, &project_name)
                .await
                .with_context(|| {
                    format!(
                        "failed to auto-start runtime for project '{}'",
                        project_name
                    )
                })?;
        }

        let state = AppState {
            project_service,
            runtime,
            executable_path: std::env::current_exe().context("unable to resolve current exe")?,
        };

        let bind: SocketAddr = format!("{host}:{port}")
            .parse()
            .context("invalid bind host/port")?;
        serve(state, bind).await
    })
}
