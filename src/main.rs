use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::{Parser, Subcommand};

use smartapc::{
    desktop::{DesktopSettings, run as run_desktop},
    project::ProjectService,
    runtime::RuntimeManager,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        } => {
            std::fs::create_dir_all(&project_root).with_context(|| {
                format!("failed to create project root {}", project_root.display())
            })?;

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
                executable_path: std::env::current_exe()
                    .context("unable to resolve current exe")?,
            };

            let bind: SocketAddr = format!("{host}:{port}")
                .parse()
                .context("invalid bind host/port")?;
            serve(state, bind).await
        }
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
    }
}
