use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::project_config::DeploymentConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentCommands {
    pub install: String,
    pub start: String,
    pub stop: String,
    pub remove: String,
}

pub fn windows_service_commands(
    deployment: &DeploymentConfig,
    executable: &Path,
    project_name: &str,
    project_root: &Path,
) -> DeploymentCommands {
    let bin_path = format!(
        "\"{}\" serve --project-root \"{}\" --project \"{}\"",
        executable.display(),
        project_root.display(),
        project_name
    );

    DeploymentCommands {
        install: format!(
            "sc.exe create {} binPath= \"{}\" DisplayName= \"{}\" start= auto",
            deployment.service_name, bin_path, deployment.display_name
        ),
        start: format!("sc.exe start {}", deployment.service_name),
        stop: format!("sc.exe stop {}", deployment.service_name),
        remove: format!("sc.exe delete {}", deployment.service_name),
    }
}
