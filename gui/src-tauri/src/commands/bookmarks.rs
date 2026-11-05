use super::super::{AppState, GuiError, SessionTarget};
use tauri::State;

#[tauri::command]
pub(crate) fn bookmarks_load(
    state: State<AppState>,
) -> Result<Vec<agent_mainframe::gui_bookmarks::BookmarkRow>, GuiError> {
    agent_mainframe::gui_bookmarks::load(&mut state.0.lock().expect("gui handle mutex poisoned"))
}
#[tauri::command]
pub(crate) fn bookmarks_add(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<Vec<agent_mainframe::gui_bookmarks::BookmarkRow>, GuiError> {
    agent_mainframe::gui_bookmarks::add(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &target,
    )
}
#[tauri::command]
pub(crate) fn bookmarks_remove(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<Vec<agent_mainframe::gui_bookmarks::BookmarkRow>, GuiError> {
    agent_mainframe::gui_bookmarks::remove(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &target,
    )
}
#[tauri::command]
pub(crate) fn bookmarks_resolve(
    state: State<AppState>,
    target: SessionTarget,
) -> Result<SessionTarget, GuiError> {
    agent_mainframe::gui_bookmarks::resolve(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
    )
}

#[tauri::command]
pub(crate) fn needs_attention_load(
    state: State<AppState>,
) -> Result<agent_mainframe::gui_attention::NeedsAttentionView, GuiError> {
    agent_mainframe::gui_attention::load(&mut state.0.lock().expect("gui handle mutex poisoned"))
}
#[tauri::command]
pub(crate) fn needs_attention_dismiss(
    state: State<AppState>,
    id: String,
    modified_ms: Option<i64>,
) -> Result<agent_mainframe::gui_attention::NeedsAttentionView, GuiError> {
    agent_mainframe::gui_attention::dismiss(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &id,
        modified_ms,
    )
}
