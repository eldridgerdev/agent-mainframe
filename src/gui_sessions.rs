//! Desktop parity with the TUI's `s` session picker beyond agents, terminals
//! and Neovim: VS Code, configured custom sessions and the per-feature TODOs
//! session.
//!
//! Every launch goes through the engine the TUI picker uses:
//!
//! - **VS Code** is `App::launch_vscode_window`, the picker's own launch:
//!   `code --new-window <worktree>`, a not-owned `launched_editors` row and
//!   the background resolver that upgrades it to owned only when a new local
//!   window appears. Stopping the feature (or *Close VS Code windows*, the
//!   TUI dormant list's `e`) runs `App::kill_tracked_editors` with all of its
//!   ownership rules, so nothing here signals a process itself. Like the TUI,
//!   opening VS Code on a stopped feature starts the feature first, which is
//!   what ties the window's cleanup to the feature's stop.
//! - **Custom sessions** come from the project's effective `amf.json` (the
//!   project file merged with the global config, as the picker reads it).
//!   The `pre_check` runs first, without the GUI lock held, and a failure is
//!   returned as an outcome with the command's output rather than an error,
//!   so the dialog can show it. The session is then created by
//!   `App::add_custom_session_identified`, which shares the window launch
//!   with the TUI's `add_custom_session_type_named`.
//! - **TODOs** is `App::add_todos_session_for_picker`.
//!
//! Each listed custom session carries a revision (a hash of its config), and
//! creation re-reads the config and refuses a session that was removed or
//! changed since the dialog listed it.
//!
//! VS Code has no `FeatureSession` row and no tmux pane: the TUI picker never
//! created one. The GUI instead lists the feature's tracked editor windows
//! ([`FeatureEditor`]) from `launched_editors`, read on every snapshot.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::app::editor_ops::{PendingLaunchState, lock_state};
use crate::app::session_ops::{
    VSCODE_OWNER_RESOLVE_TIMEOUT, session_kind_for_agent, vscode_cli_available,
};
use crate::custom_session_icons::resolve_custom_session_icon;
use crate::db::editors::LaunchedEditor;
use crate::extension::{CustomSessionConfig, ExtensionConfig};
use crate::gui_contract::{
    AddSessionResponse, FeatureTarget, GuiError, GuiHandle, GuiResult, NewSessionOption,
    SessionTarget,
};
use crate::gui_dormancy::EditorCleanupView;
use crate::project::{ProjectStatus, SessionKind};

/// Everything the New session dialog offers for one feature, in the TUI
/// picker's order.
#[derive(Debug, Clone, Serialize)]
pub struct NewSessionOptions {
    pub builtin: Vec<NewSessionOption>,
    pub custom: Vec<CustomSessionOption>,
    /// The project's config file exists but could not be read, so only the
    /// global custom sessions are listed (the picker's own fallback).
    pub config_warning: Option<String>,
    /// Adding any tmux-backed session starts the feature, with its saved
    /// agents.
    pub feature_stopped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomSessionSource {
    /// The project's `amf.json`.
    Project,
    /// The global config, merged in under a name the project does not use.
    Global,
}

/// A configured custom session as the dialog shows it.
#[derive(Debug, Clone, Serialize)]
pub struct CustomSessionOption {
    pub name: String,
    pub description: Option<String>,
    /// Plain-text `icon`.
    pub icon: Option<String>,
    /// `icon_nerd` resolved to its glyph.
    pub icon_nerd: Option<String>,
    pub command: Option<String>,
    pub working_dir: Option<String>,
    pub window_name: Option<String>,
    pub pre_check: Option<String>,
    pub on_stop: Option<String>,
    /// The TUI opens the new session's view straight away.
    pub autolaunch: bool,
    pub source: CustomSessionSource,
    /// Hash of the entry, sent back on creation to refuse a stale one.
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddCustomSessionRequest {
    pub target: FeatureTarget,
    pub name: String,
    pub revision: String,
    pub label: Option<String>,
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AddCustomSessionResponse {
    Added {
        target: SessionTarget,
        label: String,
        autolaunch: bool,
        message: String,
    },
    /// Nothing was created: the configured `pre_check` failed.
    PreCheckFailed {
        name: String,
        pre_check: String,
        /// The check's output, or why it could not run (the TUI's toast text).
        output: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenVscodeResponse {
    pub feature_id: String,
    pub workdir: String,
    /// The `launched_editors` row tracking the window, when there is a DB.
    pub editor_id: Option<String>,
    /// The feature was stopped and this request started it.
    pub started_feature: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureEditorState {
    /// Launched; the window process has not been identified yet.
    Opening,
    /// AMF opened this window and can still identify its process.
    Open,
    /// Handed to a VS Code instance AMF did not start (or a remote window
    /// with no local process): AMF never closes it.
    NotOwned,
}

/// One editor window AMF launched for a feature.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FeatureEditor {
    pub id: String,
    pub name: String,
    pub state: FeatureEditorState,
    /// Stopping the feature closes it (`kill_editor_on_stop`, and AMF's).
    pub closes_with_feature: bool,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloseEditorsResponse {
    /// Nothing AMF could close was open any more.
    pub already_closed: bool,
    pub editors: EditorCleanupView,
    pub message: String,
}

fn revision(config: &CustomSessionConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(config).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

/// The project's own custom-session names, or why its config is unreadable.
fn project_config_names(repo: &std::path::Path) -> Result<HashSet<String>, String> {
    let Some(path) = crate::extension::resolve_project_config_path(repo) else {
        return Ok(HashSet::new());
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    let config: ExtensionConfig = serde_json::from_str(&text)
        .map_err(|error| format!("{} could not be parsed: {error}", path.display()))?;
    Ok(config
        .custom_sessions
        .into_iter()
        .map(|session| session.name)
        .collect())
}

/// The feature's tracked editor windows, from its `launched_editors` rows.
/// Dead processes and launches with no identified process past the resolve
/// window are left out. Their rows remain available for cleanup on launch.
pub(crate) fn feature_editors(app: &App, rows: &[LaunchedEditor]) -> Vec<FeatureEditor> {
    let now = Utc::now();
    let resolve_window =
        chrono::Duration::from_std(VSCODE_OWNER_RESOLVE_TIMEOUT).unwrap_or_default();
    rows.iter()
        .filter_map(|row| {
            // This process's own launches: the resolver's state is the truth.
            let local = app
                .pending_editor_launches
                .iter()
                .find(|launch| launch.record_id == row.id)
                .map(|launch| *lock_state(&launch.state));
            let state = match local {
                Some(PendingLaunchState::Resolving | PendingLaunchState::Reclaim) => {
                    FeatureEditorState::Opening
                }
                _ if row.dedicated => {
                    if !crate::resources::procs::pid_alive(row.pid) {
                        return None;
                    }
                    FeatureEditorState::Open
                }
                // Another process's launch, still within its resolve window.
                None if row.pid <= 0 && now - row.started_at < resolve_window => {
                    FeatureEditorState::Opening
                }
                _ if row.pid <= 0 && now - row.started_at >= resolve_window => {
                    // There is no process whose liveness we can check. This
                    // launch record cannot claim a window is still open.
                    return None;
                }
                _ => {
                    if row.pid > 0 && !crate::resources::procs::pid_alive(row.pid) {
                        return None;
                    }
                    FeatureEditorState::NotOwned
                }
            };
            Some(FeatureEditor {
                id: row.id.clone(),
                name: row.kind.display_name().to_string(),
                closes_with_feature: app.config.kill_editor_on_stop
                    && state != FeatureEditorState::NotOwned,
                state,
                started_at: row.started_at,
            })
        })
        .collect()
}

/// The windows a close request acts on: the ones `kill_tracked_editors`
/// would close or hand to their resolver.
fn closable(editors: &[FeatureEditor], app: &App, feature_id: &str) -> Vec<String> {
    editors
        .iter()
        .filter(|editor| match editor.state {
            FeatureEditorState::Open => true,
            FeatureEditorState::Opening => app
                .pending_editor_launches
                .iter()
                .any(|launch| launch.record_id == editor.id && launch.feature_id == feature_id),
            FeatureEditorState::NotOwned => false,
        })
        .map(|editor| editor.id.clone())
        .collect()
}

/// Probe the CLI before taking the GUI lock. Call from an async command so
/// waiting for `code --version` cannot block the window's main thread.
pub fn new_session_options(
    gui: &Mutex<GuiHandle>,
    target: &FeatureTarget,
) -> GuiResult<NewSessionOptions> {
    let vscode_available = vscode_cli_available();
    gui.lock()
        .expect("gui handle mutex poisoned")
        .new_session_options(target, vscode_available)
}

/// Check the slow CLI before locking; the launch only spawns the process
/// and resolves window ownership in the background.
pub fn open_vscode(
    gui: &Mutex<GuiHandle>,
    target: FeatureTarget,
    approved: bool,
) -> GuiResult<OpenVscodeResponse> {
    if !vscode_cli_available() {
        return Err(GuiError::conflict(
            "VS Code's `code` command was not found in PATH",
        ));
    }
    gui.lock()
        .expect("gui handle mutex poisoned")
        .open_vscode(target, approved)
}

impl GuiHandle {
    /// The TUI picker's choices for this feature: allowed agents, Terminal,
    /// Neovim, VS Code (disabled without the `code` CLI), TODOs while the
    /// feature has none, then the configured custom sessions.
    fn new_session_options(
        &mut self,
        target: &FeatureTarget,
        vscode_available: bool,
    ) -> GuiResult<NewSessionOptions> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(target)?;
        let app = self.app_for_workflow();
        let project = &app.store.projects[pi];
        let feature = &project.features[fi];
        let repo = project.repo.clone();
        let feature_stopped = feature.status == ProjectStatus::Stopped
            || !app.tmux.session_exists(&feature.tmux_session);
        let has_todos = feature.has_todos_session();

        let option = |kind, label: &str| NewSessionOption {
            kind,
            label: label.to_string(),
            disabled: None,
        };
        let mut builtin = app
            .allowed_agents_for_repo(&repo)
            .into_iter()
            .map(|agent| option(session_kind_for_agent(&agent), agent.display_name()))
            .collect::<Vec<_>>();
        builtin.push(option(SessionKind::Terminal, "Terminal"));
        builtin.push(option(SessionKind::Nvim, "Neovim"));
        builtin.push(NewSessionOption {
            disabled: (!vscode_available).then(|| "code not found in PATH".to_string()),
            ..option(SessionKind::Vscode, "VS Code")
        });
        if !has_todos {
            builtin.push(option(SessionKind::Todos, "TODOs"));
        }

        let (project_names, config_warning) = match project_config_names(&repo) {
            Ok(names) => (names, None),
            Err(warning) => (
                HashSet::new(),
                Some(format!(
                    "{warning}. Only global custom sessions are listed."
                )),
            ),
        };
        let custom = app
            .extension_for_repo(&repo)
            .custom_sessions
            .iter()
            .map(|config| CustomSessionOption {
                name: config.name.clone(),
                description: config.description.clone(),
                icon: config.icon.clone(),
                icon_nerd: config
                    .icon_nerd
                    .as_deref()
                    .map(|icon| resolve_custom_session_icon(icon).to_string()),
                command: config.command.clone(),
                working_dir: config
                    .working_dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().into_owned()),
                window_name: config.window_name.clone(),
                pre_check: config.pre_check.clone().filter(|check| !check.is_empty()),
                on_stop: config.on_stop.clone(),
                autolaunch: config.autolaunch.unwrap_or(false),
                source: if project_names.contains(&config.name) {
                    CustomSessionSource::Project
                } else {
                    CustomSessionSource::Global
                },
                revision: revision(config),
            })
            .collect();

        Ok(NewSessionOptions {
            builtin,
            custom,
            config_warning,
            feature_stopped,
        })
    }

    /// Validate a custom-session request against the current store and
    /// config: the feature, its tmux ownership, and the exact entry listed.
    fn resolve_custom_session(
        &mut self,
        request: &AddCustomSessionRequest,
    ) -> GuiResult<(usize, usize, CustomSessionConfig, PathBuf)> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&request.target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let app = self.app_for_workflow();
        if app.block_if_feature_pending_worktree_script(pi, fi) {
            app.message = None;
            return Err(GuiError::conflict(
                "Wait for the feature's worktree setup to finish before adding a session",
            ));
        }
        let repo = app.store.projects[pi].repo.clone();
        let config = app
            .extension_for_repo(&repo)
            .custom_sessions
            .into_iter()
            .find(|config| config.name == request.name)
            .ok_or_else(|| {
                GuiError::not_found(format!(
                    "'{}' is no longer a configured custom session; reopen New session",
                    request.name
                ))
            })?;
        if revision(&config) != request.revision {
            return Err(GuiError::conflict(format!(
                "'{}' changed in the project configuration since it was listed; review it and retry",
                request.name
            )));
        }
        let workdir = &app.store.projects[pi].features[fi].workdir;
        let check_dir = config
            .working_dir
            .as_ref()
            .map(|rel| workdir.join(rel))
            .unwrap_or_else(|| workdir.clone());
        Ok((pi, fi, config, check_dir))
    }

    fn create_custom_session(
        &mut self,
        request: AddCustomSessionRequest,
        checked_dir: &std::path::Path,
    ) -> GuiResult<AddCustomSessionResponse> {
        let (pi, fi, config, check_dir) = self.resolve_custom_session(&request)?;
        if check_dir != checked_dir {
            return Err(GuiError::conflict(
                "The feature's checkout moved while its pre_check ran; retry",
            ));
        }
        let label = request
            .label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| config.name.clone());
        let app = self.app_for_workflow();
        if !request.approved && app.add_would_start_feature(pi, fi, &SessionKind::Custom) {
            self.require_start_approval(&format!("Adding '{label}'"))?;
        }
        let app = self.app_for_workflow();
        let session_id = app
            .add_custom_session_identified(pi, fi, &config, label.clone())
            .map_err(GuiError::from)?;
        app.message = None;
        Ok(AddCustomSessionResponse::Added {
            target: SessionTarget {
                project_id: request.target.project_id,
                feature_id: request.target.feature_id,
                session_id,
            },
            message: format!("Added '{label}'"),
            label,
            autolaunch: config.autolaunch.unwrap_or(false),
        })
    }

    /// The picker's TODOs entry: the feature's native TODOs session and the
    /// list it opens on. One per feature.
    pub(crate) fn add_todos_session(
        &mut self,
        target: FeatureTarget,
        label: Option<String>,
    ) -> GuiResult<AddSessionResponse> {
        let (pi, fi) = self.locate(&target)?;
        let app = self.app_for_workflow();
        if app.store.projects[pi].features[fi].has_todos_session() {
            return Err(GuiError::conflict(
                "This feature already has a TODOs session",
            ));
        }
        let label = label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty());
        app.add_todos_session_for_picker(pi, fi, label)
            .map_err(GuiError::from)?;
        app.message = None;
        let session = app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
            .and_then(|(pi, fi)| app.store.projects[pi].features[fi].todos_session())
            .cloned()
            .ok_or_else(|| GuiError::conflict("The TODOs session was not saved; refresh"))?;
        Ok(AddSessionResponse {
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id: session.id,
            },
            label: session.label,
        })
    }

    /// The picker's VS Code entry. A stopped feature is started first (past
    /// the resource gate), as the TUI does; a launch still resolving for
    /// this feature refuses a second one.
    fn open_vscode(
        &mut self,
        target: FeatureTarget,
        approved: bool,
    ) -> GuiResult<OpenVscodeResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let app = self.app_for_workflow();
        if app.block_if_feature_pending_worktree_script(pi, fi) {
            app.message = None;
            return Err(GuiError::conflict(
                "Wait for the feature's worktree setup to finish before opening VS Code",
            ));
        }
        app.prune_resolved_editor_launches();
        let feature_id = app.store.projects[pi].features[fi].id.clone();
        if app.pending_editor_launches.iter().any(|launch| {
            launch.feature_id == feature_id
                && *lock_state(&launch.state) == PendingLaunchState::Resolving
        }) {
            return Err(GuiError::conflict(
                "VS Code is still opening for this feature; wait for its window",
            ));
        }
        let starts_feature = app.add_would_start_feature(pi, fi, &SessionKind::Vscode);
        if !approved && starts_feature {
            let name = app.store.projects[pi].features[fi].name.clone();
            self.require_start_approval(&format!("Opening VS Code starts '{name}', which"))?;
        }
        let app = self.app_for_workflow();
        let launch = app
            .with_feature_started_for_add(pi, fi, |app| app.launch_vscode_window(pi, fi))
            .map_err(GuiError::from)?;
        app.message = None;
        Ok(OpenVscodeResponse {
            feature_id: target.feature_id,
            workdir: launch.workdir.to_string_lossy().into_owned(),
            editor_id: launch.record_id,
            started_feature: starts_feature,
            message: format!("Opened VS Code in {}", launch.workdir.display()),
        })
    }

    /// Close the VS Code windows AMF opened for a feature: the TUI dormant
    /// list's `e`, through `App::kill_tracked_editors` and its ownership
    /// rules. `seen` is the editor rows the user confirmed; a window opened
    /// since then is not closed on the strength of an older list.
    pub fn close_editors(
        &mut self,
        target: FeatureTarget,
        seen: Vec<String>,
    ) -> GuiResult<CloseEditorsResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        let app = self.app_for_workflow();
        app.prune_resolved_editor_launches();
        let feature = &app.store.projects[pi].features[fi];
        let (feature_id, name) = (feature.id.clone(), feature.name.clone());
        let rows = match &app.db {
            Some(db) => db
                .launched_editors_for_feature(&feature_id)
                .map_err(GuiError::from)?,
            None => Vec::new(),
        };
        let editors = feature_editors(app, &rows);
        let closable = closable(&editors, app, &feature_id);
        if closable.iter().any(|id| !seen.contains(id)) {
            return Err(GuiError::conflict(
                "Another VS Code window was opened for this feature since this list loaded; review it and retry",
            ));
        }
        if closable.is_empty() {
            return Ok(CloseEditorsResponse {
                already_closed: true,
                editors: EditorCleanupView::from(
                    &crate::app::editor_ops::EditorKillReport::default(),
                ),
                message: format!("No VS Code window AMF can close is open for '{name}'"),
            });
        }
        let report = app.kill_tracked_editors(&feature_id);
        // The TUI's status-line summary, as a sentence.
        let message = report
            .summary()
            .map(|summary| {
                let mut chars = summary.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| format!("No VS Code window AMF can close is open for '{name}'"));
        Ok(CloseEditorsResponse {
            already_closed: false,
            editors: EditorCleanupView::from(&report),
            message,
        })
    }
}

/// Add a configured custom session. The `pre_check` runs between two short
/// holds of the GUI lock, so a slow check never blocks terminal input or
/// other commands; creation re-validates everything the check was run for.
pub fn add_custom_session(
    gui: &Mutex<GuiHandle>,
    request: AddCustomSessionRequest,
) -> GuiResult<AddCustomSessionResponse> {
    let key = (request.target.feature_id.clone(), request.name.clone());
    let (config, check_dir) = {
        let mut gui = gui.lock().expect("gui handle mutex poisoned");
        let (_, _, config, check_dir) = gui.resolve_custom_session(&request)?;
        if !gui.custom_session_adds.insert(key.clone()) {
            return Err(GuiError::conflict(format!(
                "'{}' is already being added to this feature; wait for its pre-check",
                request.name
            )));
        }
        (config, check_dir)
    };
    // Release the reservation on every outcome, including pre-check failure,
    // approval and stale-target errors. The check never holds the GUI lock.
    let reservation = CustomSessionAdd { gui, key };
    if let Err(output) = config.run_pre_check(&check_dir) {
        return Ok(AddCustomSessionResponse::PreCheckFailed {
            name: config.name.clone(),
            pre_check: config.pre_check.clone().unwrap_or_default(),
            output,
        });
    }
    let result = gui
        .lock()
        .expect("gui handle mutex poisoned")
        .create_custom_session(request, &check_dir);
    drop(reservation);
    result
}

struct CustomSessionAdd<'a> {
    gui: &'a Mutex<GuiHandle>,
    key: (String, String),
}

impl Drop for CustomSessionAdd<'_> {
    fn drop(&mut self) {
        if let Ok(mut gui) = self.gui.lock() {
            gui.custom_session_adds.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests;
