//! Project preference is SQLite state, independent of allowed_agents JSON.
//!
//! Harness probes launch a `--version`/`--help` process per harness, so they
//! run outside the GUI lock: a short hold reads everything else, the probes
//! run unlocked and in parallel, and a save re-reads under the lock and
//! refuses if anything the probes were run for has changed.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::{App, AppConfig, AppMode};
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

/// Reports whether a harness is installed. Version/help only: it never
/// starts a harness.
type Probe<'a> = dyn Fn(&AgentKind) -> bool + Sync + 'a;

fn installed(agent: &AgentKind) -> bool {
    App::check_harness_available(agent).is_ok()
}

/// Everything a view needs apart from which harnesses are installed.
#[derive(Debug, PartialEq)]
struct Snapshot {
    project_id: String,
    project_name: String,
    repo: PathBuf,
    physical: PathBuf,
    identity: String,
    preferred_agent: AgentKind,
    configuration_revision: String,
    global_revision: String,
    /// Allowed harnesses, before probing.
    candidates: Vec<AgentKind>,
}

fn lock(gui: &Mutex<GuiHandle>) -> std::sync::MutexGuard<'_, GuiHandle> {
    gui.lock().expect("gui handle mutex poisoned")
}

fn snapshot(gui: &mut GuiHandle, project_id: &str, global_path: &Path) -> GuiResult<Snapshot> {
    gui.refresh_store()?;
    let (global, global_revision) = crate::gui_settings::read_json_document(global_path)?;
    serde_json::from_value::<AppConfig>(global).map_err(anyhow::Error::from)?;
    let configuration = crate::gui_project_settings::load(gui, project_id)?;
    let app = gui.app_for_workflow();
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
    Ok(Snapshot {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        repo: project.repo.clone(),
        physical,
        identity,
        preferred_agent: project.preferred_agent.clone(),
        configuration_revision: configuration.revision,
        global_revision,
        candidates: app.allowed_agents_for_repo(&project.repo),
    })
}

/// Probe every candidate at once, keeping the candidates' order.
fn probe_all(candidates: &[AgentKind], probe: &Probe<'_>) -> Vec<AgentKind> {
    std::thread::scope(|scope| {
        let checks: Vec<_> = candidates
            .iter()
            .map(|agent| scope.spawn(move || probe(agent)))
            .collect();
        candidates
            .iter()
            .zip(checks)
            .filter_map(|(agent, check)| {
                // A probe that panicked counts as not installed.
                check.join().unwrap_or(false).then(|| agent.clone())
            })
            .collect()
    })
}

fn view(snapshot: &Snapshot, installed: &[AgentKind]) -> GuiResult<ProjectHarnessChoices> {
    let agents: Vec<_> = snapshot
        .candidates
        .iter()
        .filter(|agent| installed.contains(agent))
        .cloned()
        .collect();
    let default_agent = AgentKind::preferred_in(&agents, &snapshot.preferred_agent);
    let stamp = serde_json::to_vec(&(
        &snapshot.project_id,
        &snapshot.project_name,
        &snapshot.repo,
        &snapshot.physical,
        &snapshot.identity,
        &snapshot.preferred_agent,
        &snapshot.configuration_revision,
        &snapshot.global_revision,
        &agents,
    ))
    .map_err(anyhow::Error::from)?;
    Ok(ProjectHarnessChoices {
        project_id: snapshot.project_id.clone(),
        project_name: snapshot.project_name.clone(),
        repo: snapshot.repo.to_string_lossy().into_owned(),
        settings: ProjectHarnessPreference {
            preferred_agent: snapshot.preferred_agent.clone(),
        },
        agents,
        default_agent,
        revision: format!("{:x}", Sha256::digest(stamp)),
    })
}

/// Read without the active-workflow check: the creation forms use these
/// choices too, and reading changes nothing.
fn read(
    gui: &Mutex<GuiHandle>,
    project_id: &str,
    global_path: &Path,
    probe: &Probe<'_>,
) -> GuiResult<ProjectHarnessChoices> {
    let snapshot = snapshot(&mut lock(gui), project_id, global_path)?;
    let installed = probe_all(&snapshot.candidates, probe);
    view(&snapshot, &installed)
}

pub fn load(gui: &Mutex<GuiHandle>, project_id: &str) -> GuiResult<ProjectHarnessChoices> {
    read(
        gui,
        project_id,
        &crate::project::amf_config_dir().join("config.json"),
        &installed,
    )
}

fn save_at(
    gui: &Mutex<GuiHandle>,
    project_id: &str,
    revision: &str,
    next: AgentKind,
    global_path: &Path,
    probe: &Probe<'_>,
) -> GuiResult<ProjectHarnessChoices> {
    let probed = snapshot(&mut lock(gui), project_id, global_path)?;
    let installed = probe_all(&probed.candidates, probe);
    let mut gui = lock(gui);
    if !matches!(gui.app_for_workflow().mode, AppMode::Normal) {
        return Err(GuiError::conflict(
            "Finish or close the active workflow before editing project preferences",
        ));
    }
    let current = snapshot(&mut gui, project_id, global_path)?;
    if current != probed || view(&current, &installed)?.revision != revision {
        return Err(GuiError::conflict(
            "Project, preference or harness choices changed. Reload before saving.",
        ));
    }
    if !installed.contains(&next) {
        return Err(GuiError::conflict(
            "Choose an allowed, installed harness. Reload the choices.",
        ));
    }
    // The shared setter re-checks the one chosen harness under the lock, so a
    // removal since the probes above still refuses the save.
    gui.app_for_workflow()
        .set_project_preferred_harness(project_id, next)?;
    // A fresh read of the saved state. Installation was probed moments ago,
    // so it is reused rather than probed again under the lock.
    snapshot(&mut gui, project_id, global_path)
        .and_then(|saved| view(&saved, &installed))
        .map_err(|error| GuiError {
            kind: error.kind,
            message: format!(
                "Preference was saved, but refreshing choices failed: {}",
                error.message
            ),
        })
}

pub fn save(
    gui: &Mutex<GuiHandle>,
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
        &installed,
    )
}

#[cfg(test)]
mod tests;
