use super::super::GuiError;
use agent_mainframe::gui_syntax::{
    self, SyntaxInstallView, SyntaxLanguageRequest, SyntaxLanguagesView,
};

/// Progress of the GUI's parser install, polled while one runs.
#[tauri::command]
pub(crate) fn syntax_install_status() -> SyntaxInstallView {
    gui_syntax::install_status()
}

/// Installs a syntax parser with the TUI picker's installer on a background
/// thread. The frontend sends this only after the user confirms the clone
/// and compile; the shared GUI handle is not locked while it runs.
#[tauri::command]
pub(crate) fn syntax_install(language: String) -> Result<SyntaxInstallView, GuiError> {
    gui_syntax::install(&language)
}

#[tauri::command]
pub(crate) fn syntax_languages(refresh: bool) -> Result<SyntaxLanguagesView, GuiError> {
    gui_syntax::languages_list(refresh)
}

#[tauri::command]
pub(crate) fn syntax_languages_apply(
    request: SyntaxLanguageRequest,
) -> Result<SyntaxInstallView, GuiError> {
    gui_syntax::languages_apply(request)
}
