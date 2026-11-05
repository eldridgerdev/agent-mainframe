use super::super::{AppState, FeatureTarget, GuiError, SessionTarget};
use tauri::State;

#[tauri::command]
pub(crate) fn feature_actions_load(
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<agent_mainframe::gui_feature_actions::FeatureActionsView, GuiError> {
    agent_mainframe::gui_feature_actions::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
    )
}
#[tauri::command]
pub(crate) fn feature_rename(
    state: State<AppState>,
    target: FeatureTarget,
    expected: Option<String>,
    nickname: String,
) -> Result<agent_mainframe::gui_feature_actions::FeatureActionsView, GuiError> {
    agent_mainframe::gui_feature_actions::rename_feature(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        expected,
        nickname,
    )
}
#[tauri::command]
pub(crate) fn session_rename(
    state: State<AppState>,
    target: SessionTarget,
    expected: String,
    label: String,
) -> Result<agent_mainframe::gui_feature_actions::FeatureActionsView, GuiError> {
    agent_mainframe::gui_feature_actions::rename_session(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        expected,
        label,
    )
}
#[tauri::command]
pub(crate) fn feature_set_ready(
    state: State<AppState>,
    target: FeatureTarget,
    expected: bool,
) -> Result<agent_mainframe::gui_feature_actions::FeatureActionsView, GuiError> {
    agent_mainframe::gui_feature_actions::set_ready(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        expected,
    )
}
#[tauri::command]
pub(crate) fn feature_summary_notice(
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<agent_mainframe::gui_feature_actions::SummaryNotice, GuiError> {
    agent_mainframe::gui_feature_actions::summary_notice(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
    )
}
#[tauri::command]
pub(crate) fn feature_summary_generate(
    state: State<AppState>,
    target: FeatureTarget,
    ack: agent_mainframe::gui_feature_actions::SummaryAck,
) -> Result<agent_mainframe::gui_feature_actions::FeatureActionsView, GuiError> {
    agent_mainframe::gui_feature_actions::summary_generate(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        target,
        ack,
    )
}
