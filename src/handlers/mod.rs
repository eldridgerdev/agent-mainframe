mod batch_creation;
mod browse;
mod compose;
mod config_wizard;
mod dialog;
mod diff;
mod diff_review;
mod feature_creation;
mod fork;
mod harness;
mod hooks;
mod input;
mod mouse;
mod normal;
mod picker;
mod pr_review;
mod prompt_library;
mod search;
mod skill_picker;
mod todos;
mod view;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;

pub use batch_creation::handle_create_batch_features_key;
pub use browse::handle_browse_path_key;
pub use compose::handle_compose_key;
pub use config_wizard::handle_config_wizard_key;
pub use dialog::{
    handle_create_project_key, handle_debug_log_key, handle_delete_feature_key,
    handle_delete_project_key, handle_help_key, handle_latest_prompt_key,
    handle_markdown_viewer_key, handle_rename_feature_key, handle_rename_session_key,
    handle_session_config_key, handle_steering_prompt_key, handle_theme_picker_key,
};
pub use diff::{handle_diff_viewer_key, handle_review_harness_pick_key};
pub use pr_review::{
    handle_pr_number_prompt_key, handle_pr_picker_key, handle_pr_review_key,
    handle_pr_review_loading_key,
};
pub use diff_review::handle_diff_review_key;
pub use feature_creation::handle_create_feature_key;
pub use fork::handle_fork_feature_key;
pub use harness::handle_harness_setup_key;
pub use hooks::{handle_deleting_feature_key, handle_hook_prompt_key, handle_running_hook_key};
pub use input::handle_paste;
pub use mouse::handle_mouse;
pub use normal::handle_normal_key;
pub(crate) use normal::{
    DASHBOARD_KEYBINDING_ACTIONS, EXTRA_KEYBINDING_ACTIONS, default_key_for_action,
};
pub use picker::{
    handle_bookmark_picker_key, handle_claude_session_confirm_key,
    handle_claude_session_picker_key, handle_codex_session_confirm_key,
    handle_codex_session_picker_key, handle_command_picker_key, handle_markdown_file_picker_key,
    handle_new_session_name_key, handle_notification_picker_key,
    handle_opencode_session_confirm_key, handle_opencode_session_picker_key,
    handle_session_picker_key, handle_session_switcher_key, handle_syntax_language_picker_key,
};
pub use prompt_library::{
    handle_placeholder_fill_key, handle_prompt_editor_key, handle_prompt_library_key,
};
pub use search::handle_search_key;
pub use skill_picker::handle_skill_picker_key;
pub use todos::{handle_todo_quick_capture_key, handle_todos_key};
pub use view::handle_view_key;

pub fn handle_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    use crate::app::AppMode;

    match &app.mode {
        AppMode::Normal => handle_normal_key(app, key),
        AppMode::Todos(_) => handle_todos_key(app, key),
        AppMode::TodoQuickCapture(_) => handle_todo_quick_capture_key(app, key),
        AppMode::CreatingProject(_) => handle_create_project_key(app, key),
        AppMode::BrowsingPath(_) => handle_browse_path_key(app, key),
        AppMode::CreatingFeature(_) => handle_create_feature_key(app, key.code),
        AppMode::CreatingBatchFeatures(_) => handle_create_batch_features_key(app, key.code),
        AppMode::DeletingProject(_) => handle_delete_project_key(app, key.code),
        AppMode::DeletingFeature(_, _) => handle_delete_feature_key(app, key.code),
        AppMode::Viewing(_) => handle_view_key(app, key, visible_rows),
        AppMode::Help(_) => handle_help_key(app, key),
        AppMode::SteeringPrompt(_) => handle_steering_prompt_key(app, key),
        AppMode::Compose(_) => handle_compose_key(app, key),
        AppMode::NotificationPicker(_, _) => handle_notification_picker_key(app, key.code),
        AppMode::SessionSwitcher(_) => handle_session_switcher_key(app, key.code),
        AppMode::RenamingSession(_) => handle_rename_session_key(app, key.code),
        AppMode::RenamingFeature(_) => handle_rename_feature_key(app, key.code),
        AppMode::SessionConfig(_) => handle_session_config_key(app, key.code),
        AppMode::ProjectAgentConfig(_) => handle_session_config_key(app, key.code),
        AppMode::CommandPicker(_) => handle_command_picker_key(app, key.code),
        AppMode::MarkdownFilePicker(_) => handle_markdown_file_picker_key(app, key),
        AppMode::Searching(_) => handle_search_key(app, key.code),
        AppMode::NamingNewSession(_) => handle_new_session_name_key(app, key.code),
        AppMode::OpencodeSessionPicker(_) => handle_opencode_session_picker_key(app, key.code),
        AppMode::ConfirmingOpencodeSession { .. } => {
            handle_opencode_session_confirm_key(app, key.code)
        }
        AppMode::ClaudeSessionPicker(_) => handle_claude_session_picker_key(app, key.code),
        AppMode::ConfirmingClaudeSession { .. } => handle_claude_session_confirm_key(app, key.code),
        AppMode::CodexSessionPicker(_) => handle_codex_session_picker_key(app, key.code),
        AppMode::ConfirmingCodexSession { .. } => handle_codex_session_confirm_key(app, key.code),
        AppMode::SessionPicker(_) => handle_session_picker_key(app, key.code),
        AppMode::BookmarkPicker(_) => handle_bookmark_picker_key(app, key.code),
        AppMode::DiffViewerLoading(_) => handle_diff_viewer_key(app, key),
        AppMode::DiffViewer(_) => handle_diff_viewer_key(app, key),
        AppMode::PrNumberPrompt(_) => handle_pr_number_prompt_key(app, key),
        AppMode::PrPicker(_) => handle_pr_picker_key(app, key),
        AppMode::PrReviewLoading(_) => handle_pr_review_loading_key(app, key),
        AppMode::PrReview(_) => handle_pr_review_key(app, key),
        AppMode::DiffReviewPrompt(_) => handle_diff_review_key(app, key),
        AppMode::RunningHook(_) => handle_running_hook_key(app, key.code),
        AppMode::DeletingFeatureInProgress(_) => handle_deleting_feature_key(app, key.code),
        AppMode::HookPrompt(_) => handle_hook_prompt_key(app, key.code),
        AppMode::LatestPrompt(_) => handle_latest_prompt_key(app, key.code),
        AppMode::PromptLibrary(_) => handle_prompt_library_key(app, key.code),
        AppMode::PromptEditor(_) => handle_prompt_editor_key(app, key),
        AppMode::PlaceholderFill(_) => handle_placeholder_fill_key(app, key),
        AppMode::SkillPicker(_) => handle_skill_picker_key(app, key),
        AppMode::ForkingFeature(_) => handle_fork_feature_key(app, key.code),
        AppMode::ThemePicker(_) => handle_theme_picker_key(app, key.code),
        AppMode::SyntaxLanguagePicker(_) => handle_syntax_language_picker_key(app, key.code),
        AppMode::DebugLog(_) => handle_debug_log_key(app, key.code),
        AppMode::MarkdownLoading(_) => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                app.cancel_markdown_loading();
            }
            Ok(())
        }
        AppMode::MarkdownViewer(_) => handle_markdown_viewer_key(app, key),
        AppMode::HarnessSetup(_) => handle_harness_setup_key(app, key.code),
        AppMode::ConfigWizard(_) => handle_config_wizard_key(app, key),
        AppMode::ReviewHarnessPick(_) => handle_review_harness_pick_key(app, key.code),
    }
}
