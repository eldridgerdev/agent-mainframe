//! Project preference is SQLite state, independent of allowed_agents JSON.
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::{AppConfig, AppMode};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::project::AgentKind;

#[derive(Debug, Serialize)]
pub struct ProjectHarnessChoices {
    pub project_id: String,
    pub project_name: String,
    pub repo: String,
    pub settings: ProjectHarnessPreference,
    pub agents: Vec<AgentKind>,
    /// Shared preferred-or-first normalization; empty choices have no default.
    pub default_agent: Option<AgentKind>,
    pub revision: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectHarnessPreference {
    pub preferred_agent: AgentKind,
}

fn read(
    gui: &mut GuiHandle,
    project_id: &str,
    global_path: &Path,
) -> GuiResult<ProjectHarnessChoices> {
    gui.refresh_store()?;
    let (global, global_revision) = crate::gui_settings::read_json_document(global_path)?;
    serde_json::from_value::<AppConfig>(global).map_err(anyhow::Error::from)?;
    let configuration = crate::gui_project_settings::load(gui, project_id)?;
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::Normal) {
        return Err(GuiError::conflict(
            "Finish or close the active workflow before editing project preferences",
        ));
    }
    let project = app
        .store
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| GuiError::not_found("Project was deleted. Choose another project."))?;
    let physical = project.repo.canonicalize().map_err(anyhow::Error::from)?;
    let metadata = physical.metadata().map_err(anyhow::Error::from)?;
    if !metadata.is_dir() {
        return Err(GuiError::conflict(
            "Project repository is no longer a directory",
        ));
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created().ok());
    // Probe version/help only. Saving a preference never starts a harness.
    let agents: Vec<_> = app
        .allowed_agents_for_repo(&project.repo)
        .into_iter()
        .filter(|agent| app.tmux.check_harness_available(agent).is_ok())
        .collect();
    let default_agent = AgentKind::preferred_in(&agents, &project.preferred_agent);
    let stamp = serde_json::to_vec(&(
        &project.id,
        &project.name,
        &project.repo,
        &physical,
        &identity,
        &project.preferred_agent,
        &configuration.revision,
        global_revision,
        &agents,
    ))
    .map_err(anyhow::Error::from)?;
    Ok(ProjectHarnessChoices {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        repo: project.repo.to_string_lossy().into_owned(),
        settings: ProjectHarnessPreference {
            preferred_agent: project.preferred_agent.clone(),
        },
        agents,
        default_agent,
        revision: format!("{:x}", Sha256::digest(stamp)),
    })
}

pub fn load(gui: &mut GuiHandle, project_id: &str) -> GuiResult<ProjectHarnessChoices> {
    read(
        gui,
        project_id,
        &crate::project::amf_config_dir().join("config.json"),
    )
}

fn save_at(
    gui: &mut GuiHandle,
    project_id: &str,
    revision: &str,
    next: AgentKind,
    global_path: &Path,
) -> GuiResult<ProjectHarnessChoices> {
    let current = read(gui, project_id, global_path)?;
    if current.revision != revision {
        return Err(GuiError::conflict(
            "Project, preference or harness choices changed. Reload before saving.",
        ));
    }
    if !current.agents.contains(&next) {
        return Err(GuiError::conflict(
            "Choose an allowed, installed harness. Reload the choices.",
        ));
    }
    gui.app_for_workflow()
        .set_project_preferred_harness(project_id, next)?;
    // This is only a fresh read, never a second save or process operation.
    read(gui, project_id, global_path).map_err(|error| GuiError {
        kind: error.kind,
        message: format!(
            "Preference was saved, but refreshing choices failed: {}",
            error.message
        ),
    })
}

pub fn save(
    gui: &mut GuiHandle,
    project_id: &str,
    revision: &str,
    preferred_agent: AgentKind,
) -> GuiResult<ProjectHarnessChoices> {
    save_at(
        gui,
        project_id,
        revision,
        preferred_agent,
        &crate::project::amf_config_dir().join("config.json"),
    )
}

#[cfg(test)]
mod tests;
