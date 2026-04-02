use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::project_config::{ProjectConfig, SidebarTask, from_toml_str, to_toml_string};

const PROJECT_FILE: &str = "project.toml";

#[derive(Debug, Clone)]
pub struct ProjectService {
    root: PathBuf,
}

impl ProjectService {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_dir(&self, project_name: &str) -> PathBuf {
        self.root.join(project_name)
    }

    pub fn config_path(&self, project_name: &str) -> PathBuf {
        self.project_dir(project_name).join(PROJECT_FILE)
    }

    pub fn create_project(&self, project_name: &str) -> Result<ProjectConfig> {
        validate_project_name(project_name)?;

        let project_dir = self.project_dir(project_name);
        if project_dir.exists() {
            bail!("project '{}' already exists", project_name);
        }

        fs::create_dir_all(project_dir.join("data"))
            .with_context(|| format!("failed creating project dir '{}'", project_name))?;

        let cfg = ProjectConfig::new(project_name);
        cfg.validate()?;
        self.save_config(project_name, &cfg)?;

        Ok(cfg)
    }

    pub fn list_projects(&self) -> Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut projects = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    projects.push(name.to_string());
                }
            }
        }
        projects.sort();
        Ok(projects)
    }

    pub fn load_config(&self, project_name: &str) -> Result<ProjectConfig> {
        let path = self.config_path(project_name);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed reading config file '{}'", path.display()))?;
        from_toml_str(&content)
    }

    pub fn save_config(&self, project_name: &str, cfg: &ProjectConfig) -> Result<()> {
        cfg.validate()?;

        let dir = self.project_dir(project_name);
        fs::create_dir_all(&dir)?;

        let encoded = to_toml_string(cfg)?;
        fs::write(self.config_path(project_name), encoded)?;
        Ok(())
    }

    pub fn sidebar_tasks(&self, project_name: &str) -> Result<Vec<SidebarTask>> {
        let cfg = self.load_config(project_name)?;
        Ok(cfg.sidebar_tasks())
    }

    pub fn update_logic_params(
        &self,
        project_name: &str,
        updates: std::collections::HashMap<String, f64>,
    ) -> Result<ProjectConfig> {
        let mut cfg = self.load_config(project_name)?;
        cfg.logic.params.extend(updates);
        cfg.validate()?;
        self.save_config(project_name, &cfg)?;
        Ok(cfg)
    }
}

fn validate_project_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("project name must not be empty");
    }

    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');

    if !valid {
        bail!("project name may contain only letters, numbers, '_' and '-'");
    }
    Ok(())
}
