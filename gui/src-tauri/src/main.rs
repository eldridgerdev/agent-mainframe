// Tauri commands wiring `gui_contract::GuiHandle` and `gui_terminal`
// (Tasks 4-6). The commands here are thin on purpose -- lock the shared
// handle, delegate, emit a fresh snapshot on mutation -- because the actual
// behavior (id resolution, idempotency, structured errors, terminal
// lifecycle) lives in the shared library and is covered by its own
// Rust-side tests; what those tests cannot exercise is the IPC round trip
// itself (JSON serialization of the snapshot and of `GuiError`), which is
// this file's job. Task 7 ("Deliver the vertical slice") adds the real
// navigation/forms consuming these commands in `gui/src`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod login_path;

use std::sync::Mutex;

use agent_mainframe::automation::{
    AutomationHookPrompt, CreateFeatureRequest, CreateFeatureResponse, CreateProjectRequest,
    CreateProjectResponse,
};
use agent_mainframe::gui_contract::sidebar::CollapseTarget;
use agent_mainframe::gui_contract::{
    DeleteFeatureResponse, FeatureTarget, GuiError, GuiHandle, RemoveSessionResponse,
    SessionTarget, StartFeatureResponse, StartSessionResponse, StopFeatureResponse,
    StopSessionResponse, TodoAgentLaunchResponse, TodoDeleteChoice, TodoHostChoice,
    WorkspaceSnapshot,
};
use agent_mainframe::gui_debug_log::{self, DebugLogView};
use agent_mainframe::gui_diff::{self, DiffOptions, DiffView};
use agent_mainframe::gui_settings;
use agent_mainframe::gui_supervised_edits;
use agent_mainframe::project::{AgentKind, VibeMode};
use commands::session_sidebar::SidebarSources;
use serde::Serialize;
use tauri::{Emitter, Manager, State};

#[tauri::command]
fn theme_catalog() -> agent_mainframe::gui_themes::ThemeCatalog {
    agent_mainframe::gui_themes::load()
}

struct AppState(Mutex<GuiHandle>);
struct EditExplanationState(Mutex<gui_supervised_edits::explanations::Controller>);

#[tauri::command]
async fn load_diff(
    state: State<'_, AppState>,
    target: FeatureTarget,
    options: DiffOptions,
) -> Result<DiffView, GuiError> {
    gui_diff::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        options,
    )
}

#[derive(Serialize)]
struct HarnessInfo {
    slug: String,
    display_name: String,
}

impl From<&AgentKind> for HarnessInfo {
    fn from(kind: &AgentKind) -> Self {
        Self {
            slug: kind.slug().to_string(),
            display_name: kind.display_name().to_string(),
        }
    }
}

/// Returns the harnesses AMF supports, straight from the same [`AgentKind`]
/// the TUI and the automation JSON API already use.
#[tauri::command]
fn supported_harnesses() -> Vec<HarnessInfo> {
    AgentKind::ALL.iter().map(HarnessInfo::from).collect()
}

/// The harnesses a creation form may offer for `project_id`, or for a project
/// that does not exist yet: the ones creation will accept, not every
/// supported one.
#[tauri::command]
fn allowed_harnesses(
    state: State<AppState>,
    project_id: Option<String>,
) -> Result<Vec<HarnessInfo>, GuiError> {
    let allowed = gui_settings::allowed_harnesses(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        project_id.as_deref(),
    )?;
    Ok(allowed.iter().map(HarnessInfo::from).collect())
}

#[derive(Serialize)]
struct ModeInfo {
    slug: String,
    display_name: String,
    description: String,
    /// Slugs of the primary agents that cannot run in this mode, from
    /// `VibeMode::supports_agent`, so the frontend never restates the rule.
    unsupported_agents: Vec<String>,
}

/// Returns the vibe modes AMF supports. `slug` is derived from `VibeMode`'s
/// own `Serialize` impl (its wire format for `CreateFeatureRequest.mode`)
/// rather than a hand-written match, so it cannot drift from what
/// `create_feature` actually expects.
#[tauri::command]
fn supported_modes() -> Vec<ModeInfo> {
    VibeMode::ALL
        .iter()
        .map(|mode| ModeInfo {
            slug: serde_json::to_value(mode)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            display_name: mode.display_name().to_string(),
            description: mode.permission_summary().to_string(),
            unsupported_agents: AgentKind::ALL
                .iter()
                .filter(|agent| !mode.supports_agent(agent))
                .map(|agent| agent.slug().to_string())
                .collect(),
        })
        .collect()
}

/// Cadence for the sidebar's background sources (session status/context and
/// the dashboard PR sweep), kept beside the handle so tests never start them.

#[tauri::command]
fn get_snapshot(
    state: State<AppState>,
    sources: State<SidebarSources>,
) -> Result<WorkspaceSnapshot, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    gui.drive_sidebar_sources(&mut sources.0.lock().expect("sidebar clock mutex poisoned"));
    gui.refresh_live_snapshot()
}

/// Persists a sidebar row's collapse state in the store the TUI shares.
#[tauri::command]
fn set_collapsed(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: CollapseTarget,
    collapsed: bool,
) -> Result<WorkspaceSnapshot, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let snapshot = gui.set_collapsed(target, collapsed)?;
    emit_workspace_changed(&app, &snapshot);
    Ok(snapshot)
}

/// Emits the post-mutation snapshot on `workspace-changed` so any open
/// window resynchronizes without a second round trip. A snapshot-on-change
/// broadcast, not a diff -- acceptable for the first slice (see
/// `gui_contract::WorkspaceSnapshot`'s doc comment); revisioned/fine-grained
/// events are deferred.
fn emit_workspace_changed(app: &tauri::AppHandle, snapshot: &WorkspaceSnapshot) {
    if let Err(err) = app.emit("workspace-changed", snapshot) {
        eprintln!("amf-gui: failed to emit workspace-changed: {err}");
    }
}

#[tauri::command]
async fn steering_analyze(
    text: String,
) -> Result<agent_mainframe::gui_steering::SteeringAnalysisView, GuiError> {
    tauri::async_runtime::spawn_blocking(move || agent_mainframe::gui_steering::analyze(&text))
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("The steering check stopped: {e}")))?
        .map_err(|message| GuiError::from(anyhow::anyhow!(message)))
}

#[tauri::command]
async fn debug_log_load(state: State<'_, AppState>) -> Result<DebugLogView, GuiError> {
    gui_debug_log::load(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

// PR workspace state changes are usually short, but they share a mutex with
// the rest of the GUI and can contend with GitHub/process work. Keep that
// blocking lock and the transition off Tauri's async runtime workers.
async fn pr_workspace_blocking<T: Send + 'static>(
    app: tauri::AppHandle,
    work: impl FnOnce(&Mutex<GuiHandle>) -> T + Send + 'static,
) -> Result<T, GuiError> {
    tauri::async_runtime::spawn_blocking(move || work(&app.state::<AppState>().0))
        .await
        .map_err(|e| GuiError::from(anyhow::anyhow!("PR workspace operation stopped: {e}")))
}

fn main() {
    if cfg!(target_os = "macos") {
        // SAFETY: first statement of `main`, before Tauri or anything else
        // starts a thread.
        unsafe { login_path::adopt_login_shell_path() };
    }
    // After the PATH adoption above, which the `amf` lookup reads.
    agent_mainframe::gui_contract::use_cli_for_session_hooks();
    agent_mainframe::gui_contract::use_configured_tmux_server();
    tauri::Builder::default()
        // Native folder/file pickers for the Browse… buttons; only
        // `dialog:allow-open` is granted (capabilities/default.json).
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db_path = agent_mainframe::project::db_path();
            let gui = GuiHandle::new(db_path)?;
            app.manage(AppState(Mutex::new(gui)));
            app.manage(EditExplanationState(Mutex::new(
                gui_supervised_edits::explanations::Controller::default(),
            )));
            app.manage(commands::terminal::TerminalState::new());
            app.manage(commands::session_sidebar::SidebarSources::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bookmarks::bookmarks_load,
            commands::bookmarks::bookmarks_add,
            commands::bookmarks::bookmarks_remove,
            commands::bookmarks::bookmarks_resolve,
            commands::bookmarks::needs_attention_load,
            commands::bookmarks::needs_attention_dismiss,
            commands::feature_actions::feature_actions_load,
            commands::feature_actions::feature_rename,
            commands::feature_actions::session_rename,
            commands::feature_actions::feature_set_ready,
            commands::feature_actions::feature_summary_notice,
            commands::feature_actions::feature_summary_generate,
            theme_catalog,
            commands::screenshots::screenshots_pr_document,
            commands::screenshots::screenshots_inline_image,
            commands::screenshots::screenshots_check_access,
            commands::screenshots::screenshots_remote_list,
            commands::screenshots::screenshots_remote_image,
            commands::screenshots::screenshots_remote_close,
            commands::screenshots::screenshots_open_browser,
            commands::screenshots::screenshots_list,
            commands::screenshots::screenshots_image,
            commands::screenshots::screenshots_changed,
            commands::screenshots::screenshots_cleanup_scopes,
            commands::screenshots::screenshots_cleanup,
            commands::prompts::prompt_library_load,
            commands::prompts::prompt_library_resolve,
            commands::prompts::prompt_user_edit,
            commands::prompts::prompt_user_reload,
            commands::prompts::prompt_user_save,
            commands::prompts::prompt_user_delete,
            commands::prompts::prompt_user_duplicate,
            commands::prompts::prompt_file_edit,
            commands::prompts::prompt_file_reload,
            commands::prompts::prompt_file_save,
            commands::prompts::prompt_export_preview,
            commands::prompts::prompt_export_commit,
            commands::prompts::prompt_overrides_load,
            commands::prompts::prompt_overrides_save,
            commands::prompts::prompt_overrides_clear,
            commands::prompts::prompt_overrides_precall_target,
            commands::review::review_begin,
            commands::review::review_snapshot,
            commands::review::review_destination_catalog,
            commands::review::review_destination_preview,
            commands::review::review_act,
            commands::review::review_take_completion,
            load_diff,
            commands::settings::diff_layout_save,
            supported_harnesses,
            allowed_harnesses,
            supported_modes,
            get_snapshot,
            set_collapsed,
            commands::features::create_project,
            steering_analyze,
            commands::features::create_feature,
            commands::features::worktree_hook_prompt,
            commands::sessions::start_feature,
            commands::issues::issue_browser_list,
            commands::issues::issue_fix_preview,
            commands::issues::issue_fix_create,
            commands::sessions::new_session_options,
            commands::sessions::add_session,
            commands::sessions::start_session,
            commands::sessions::stop_session,
            commands::sessions::remove_session,
            commands::sessions::delete_feature,
            commands::sessions::set_feature_vibe_mode,
            commands::features::feature_harness_load,
            commands::features::feature_harness_apply,
            commands::sessions::session_recovery_option,
            commands::sessions::saved_agent_sessions,
            commands::sessions::recover_session,
            commands::sessions::stop_feature,
            commands::terminal::attach_terminal,
            commands::terminal::terminal_input,
            commands::terminal::terminal_submit_prompt,
            commands::terminal::resize_terminal,
            commands::terminal::detach_terminal,
            commands::terminal::terminal_history,
            commands::terminal::terminal_wheel,
            commands::terminal::terminal_refresh,
            commands::todos::todo_list,
            commands::todos::todo_add,
            commands::todos::todo_set_status,
            commands::todos::todo_update,
            commands::todos::todo_update_scratchpad,
            commands::todos::todo_work_target,
            commands::todos::todo_next_preview,
            commands::todos::todo_quick_capture_target,
            commands::todos::todo_quick_capture,
            commands::todos::todo_delete,
            commands::todos::todo_move,
            commands::todos::todo_copy,
            commands::todos::todo_reorder,
            commands::todos::todo_launch_agent,
            commands::todos::todo_launch_new_feature,
            commands::learning::learning_begin,
            commands::learning::learning_snapshot,
            commands::learning::learning_act,
            commands::learning::learning_launch_agent,
            commands::plans::plan_begin,
            commands::plans::plan_begin_todo_host,
            commands::plans::plan_begin_creation,
            commands::plans::plan_begin_todo_new,
            commands::plans::plan_snapshot,
            commands::plans::plan_act,
            commands::supervised_edits::supervised_edit_explanation_prepare,
            commands::supervised_edits::supervised_edit_explanation_start,
            commands::supervised_edits::supervised_edit_explanation_poll,
            commands::supervised_edits::supervised_edit_explanation_cancel,
            commands::supervised_edits::supervised_edits_load,
            commands::supervised_edits::supervised_edit_counts,
            commands::supervised_edits::supervised_edit_respond,
            commands::settings::feature_presets_load,
            commands::settings::feature_presets_save,
            commands::settings::feature_presets_choices,
            commands::settings::context_settings_load,
            commands::settings::context_settings_save,
            commands::settings::agent_availability_load,
            commands::settings::agent_availability_save,
            commands::settings::project_agent_availability_load,
            commands::settings::project_agent_availability_save,
            commands::settings::project_harness_choices,
            commands::settings::project_preference_save,
            commands::settings::dormancy_settings_load,
            commands::settings::dormancy_settings_save,
            commands::settings::dormancy_load,
            debug_log_load,
            commands::settings::dormancy_stop,
            commands::settings::dormancy_delete,
            commands::fork::fork_prepare,
            commands::fork::fork_confirm,
            commands::fork::fork_cancel,
            commands::fork::fork_status,
            commands::pull_requests::pr_review_begin,
            commands::pull_requests::pr_review_snapshot,
            commands::pull_requests::pr_review_act,
            commands::pull_requests::pr_triage_begin,
            commands::pull_requests::pr_triage_snapshot,
            commands::pull_requests::pr_triage_act,
            commands::companion_integration::companion_integration_status,
            commands::companion_integration::companion_integration_preview,
            commands::companion_integration::companion_integration_confirm,
            commands::pr_workspace::pr_workspace_begin,
            commands::pr_workspace::pr_workspace_select_pr,
            commands::pr_workspace::pr_workspace_switch_ai,
            commands::pr_workspace::pr_workspace_switch_threads,
            commands::pr_workspace::pr_workspace_close,
            commands::pr_workspace::pr_workspace_ai_snapshot,
            commands::pr_workspace::pr_workspace_ai_act,
            commands::pr_workspace::pr_workspace_files,
            commands::pr_workspace::pr_workspace_review_open,
            commands::pr_workspace::pr_workspace_review_close,
            commands::pr_workspace::pr_workspace_prepare_fix,
            commands::pr_workspace::pr_dedicated_catalog,
            commands::pr_destinations::pr_destination_preview,
            commands::pr_destinations::pr_destination_prepare,
            commands::pr_destinations::pr_companion_catalog,
            commands::pr_destinations::pr_destination_status,
            commands::pr_destinations::pr_destination_cancel,
            commands::pr_workspace::pr_workspace_fix_targets,
            commands::pr_destinations::pr_destination_catalog,
            commands::syntax::syntax_install_status,
            commands::syntax::syntax_install,
            commands::syntax::syntax_languages,
            commands::syntax::syntax_languages_apply,
            commands::session_sidebar::session_sidebar,
            commands::session_sidebar::session_sidebar_plan,
            commands::session_sidebar::fresh_context_preview,
            commands::session_sidebar::fresh_context_start,
            commands::session_sidebar::session_sidebar_complete_todo,
            commands::sessions::add_custom_session,
            commands::sessions::open_vscode,
            commands::sessions::close_editors,
        ])
        .run(tauri::generate_context!())
        .expect("error while running amf-gui");
}

#[cfg(test)]
mod tests {

    /// The shapes `forkApi.ts` sends and expects, through the same types the
    /// Tauri commands (de)serialize, so a drifted field fails here and not at
    /// the first click.
    #[test]
    fn fork_commands_use_the_wire_shapes_of_the_frontend_client() {
        use agent_mainframe::gui_fork::{ForkConfirmRequest, ForkOutcome, ForkPrepareRequest};
        let prepare: ForkPrepareRequest = serde_json::from_value(serde_json::json!({
            "target": {"project_id": "p", "feature_id": "f"}, "session_id": "s"
        }))
        .unwrap();
        assert_eq!(prepare.session_id.as_deref(), Some("s"));
        let confirm: ForkConfirmRequest = serde_json::from_value(serde_json::json!({
            "token": "t",
            "expected": {"project_id": "p", "feature_id": "f", "session_id": null, "branch": "b", "head": "h"},
            "new_branch": "n", "agent": "codex", "include_context": false,
            "hook_choice": "Continue", "start_agent": true, "approve_resources": true
        }))
        .unwrap();
        assert!(confirm.start_agent && confirm.approve_resources);
        assert!(
            serde_json::from_value::<ForkConfirmRequest>(serde_json::json!({"token": "t"}))
                .is_err()
        );
        let cancelled = serde_json::to_value(ForkOutcome::Cancelled {
            message: "m".into(),
        })
        .unwrap();
        assert_eq!(cancelled["status"], "cancelled");
        let error = serde_json::to_value(agent_mainframe::gui_contract::GuiError::from(
            anyhow::anyhow!("boom"),
        ))
        .unwrap();
        assert_eq!(error["kind"], "internal");
    }
}
