mod normal;
mod view;
mod dialog;
mod picker;
mod input;
mod search;
mod change_reason;

use anyhow::Result;
use crossterm::event::KeyEvent;

use crate::app::App;

pub use normal::handle_normal_key;
pub use view::handle_view_key;
pub use dialog::{
    handle_create_project_key,
    handle_create_feature_key,
    handle_delete_project_key,
    handle_delete_feature_key,
    handle_help_key,
    handle_browse_path_key,
    handle_rename_session_key,
    handle_running_hook_key,
};
pub use picker::{
    handle_command_picker_key,
    handle_notification_picker_key,
    handle_session_switcher_key,
    handle_opencode_session_picker_key,
    handle_opencode_session_confirm_key,
    handle_session_picker_key,
};
pub use input::handle_paste;
pub use search::handle_search_key;
pub use change_reason::handle_change_reason_key;

pub fn handle_key(
    app: &mut App,
    key: KeyEvent,
    visible_rows: u16,
) -> Result<()> {
    use crate::app::AppMode;
    
    match &app.mode {
        AppMode::Normal => handle_normal_key(app, key),
        AppMode::CreatingProject(_) => {
            handle_create_project_key(app, key)
        }
        AppMode::BrowsingPath(_) => {
            handle_browse_path_key(app, key)
        }
        AppMode::CreatingFeature(_) => {
            handle_create_feature_key(app, key.code)
        }
        AppMode::DeletingProject(_) => {
            handle_delete_project_key(app, key.code)
        }
        AppMode::DeletingFeature(_, _) => {
            handle_delete_feature_key(app, key.code)
        }
        AppMode::Viewing(_) => handle_view_key(app, key, visible_rows),
        AppMode::Help => handle_help_key(app, key.code),
        AppMode::NotificationPicker(_) => {
            handle_notification_picker_key(app, key.code)
        }
        AppMode::SessionSwitcher(_) => {
            handle_session_switcher_key(app, key.code)
        }
        AppMode::RenamingSession(_) => {
            handle_rename_session_key(app, key.code)
        }
        AppMode::CommandPicker(_) => {
            handle_command_picker_key(app, key.code)
        }
        AppMode::Searching(_) => {
            handle_search_key(app, key.code)
        }
        AppMode::OpencodeSessionPicker(_) => {
            handle_opencode_session_picker_key(app, key.code)
        }
        AppMode::ConfirmingOpencodeSession { .. } => {
            handle_opencode_session_confirm_key(app, key.code)
        }
        AppMode::SessionPicker(_) => {
            handle_session_picker_key(app, key.code)
        }
        AppMode::ChangeReasonPrompt(_) => {
            handle_change_reason_key(app, key)
        }
        AppMode::RunningHook(_) => {
            handle_running_hook_key(app, key.code)
        }
    }
}
