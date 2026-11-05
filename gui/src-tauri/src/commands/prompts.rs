use super::super::{AppState, GuiError};
use agent_mainframe::gui_prompt_overrides::{
    self, ClearOverride, OverrideContext, OverridesView, PrecallOverrideTarget, SaveOverride,
};
use agent_mainframe::gui_prompts::{self, LibraryScope, LibraryView, ResolvePrompt};
use agent_mainframe::project::AgentKind;
use tauri::State;

#[tauri::command]
pub(crate) fn prompt_library_load(
    state: State<AppState>,
    scope: LibraryScope,
    query: String,
) -> Result<LibraryView, GuiError> {
    gui_prompts::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &scope,
        &query,
    )
}

#[tauri::command]
pub(crate) fn prompt_library_resolve(
    state: State<AppState>,
    request: ResolvePrompt,
) -> Result<String, GuiError> {
    gui_prompts::resolve(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_user_edit(
    state: State<AppState>,
    scope: LibraryScope,
    entry_key: String,
) -> Result<gui_prompts::authoring::UserPromptEditor, GuiError> {
    gui_prompts::authoring::edit(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &scope,
        &entry_key,
    )
}
#[tauri::command]
pub(crate) fn prompt_user_reload(
    state: State<AppState>,
    id: String,
) -> Result<gui_prompts::authoring::UserPromptEditor, GuiError> {
    gui_prompts::authoring::reload(&state.0.lock().expect("gui handle mutex poisoned"), &id)
}
#[tauri::command]
pub(crate) fn prompt_user_save(
    state: State<AppState>,
    draft: gui_prompts::authoring::UserPromptDraft,
) -> Result<String, GuiError> {
    gui_prompts::authoring::save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        draft,
    )
}
#[tauri::command]
pub(crate) fn prompt_user_delete(
    state: State<AppState>,
    id: String,
    revision: String,
) -> Result<(), GuiError> {
    gui_prompts::authoring::delete(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &id,
        &revision,
    )
}
#[tauri::command]
pub(crate) fn prompt_user_duplicate(
    state: State<AppState>,
    scope: LibraryScope,
    entry_key: String,
) -> Result<String, GuiError> {
    gui_prompts::authoring::duplicate(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &scope,
        &entry_key,
    )
}

#[tauri::command]
pub(crate) fn prompt_file_edit(
    state: State<AppState>,
    scope: LibraryScope,
    entry_key: String,
) -> Result<gui_prompts::declarative::DeclarativeEditor, GuiError> {
    gui_prompts::declarative::edit(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        scope,
        &entry_key,
    )
}

#[tauri::command]
pub(crate) fn prompt_file_reload(
    state: State<AppState>,
    request: gui_prompts::declarative::ReloadRequest,
) -> Result<gui_prompts::declarative::DeclarativeEditor, GuiError> {
    gui_prompts::declarative::reload(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_file_save(
    state: State<AppState>,
    request: gui_prompts::declarative::SaveRequest,
) -> Result<(), GuiError> {
    gui_prompts::declarative::save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_export_preview(
    state: State<AppState>,
    request: gui_prompts::declarative::ExportRequest,
) -> Result<gui_prompts::declarative::ExportPreview, GuiError> {
    gui_prompts::declarative::export_preview(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_export_commit(
    state: State<AppState>,
    request: gui_prompts::declarative::ExportCommit,
) -> Result<(), GuiError> {
    gui_prompts::declarative::export_commit(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_overrides_load(
    state: State<AppState>,
    context: OverrideContext,
    harness: Option<AgentKind>,
) -> Result<OverridesView, GuiError> {
    gui_prompt_overrides::load(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &context,
        harness,
    )
}

#[tauri::command]
pub(crate) fn prompt_overrides_save(
    state: State<AppState>,
    request: SaveOverride,
) -> Result<OverridesView, GuiError> {
    gui_prompt_overrides::save(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_overrides_clear(
    state: State<AppState>,
    request: ClearOverride,
) -> Result<OverridesView, GuiError> {
    gui_prompt_overrides::clear(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        request,
    )
}

#[tauri::command]
pub(crate) fn prompt_overrides_precall_target(
    state: State<AppState>,
) -> Result<PrecallOverrideTarget, GuiError> {
    gui_prompt_overrides::precall_target(&mut state.0.lock().expect("gui handle mutex poisoned"))
}
