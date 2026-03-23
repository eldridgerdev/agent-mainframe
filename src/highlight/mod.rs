mod detect;
mod model;
mod service;
mod theme;
mod tree_sitter;

pub(crate) use detect::{HighlightInstallState, HighlightLanguage};
pub use theme::style_for_class;

pub(crate) use model::{HighlightRequest, HighlightedLine, HighlightedText};
pub(crate) use service::highlight_source;

pub(crate) fn install_language<F>(
    language: HighlightLanguage,
    progress: F,
) -> anyhow::Result<String>
where
    F: FnMut(String),
{
    tree_sitter::install_language(language, progress)
}

pub(crate) fn uninstall_language<F>(
    language: HighlightLanguage,
    progress: F,
) -> anyhow::Result<String>
where
    F: FnMut(String),
{
    tree_sitter::uninstall_language(language, progress)
}

pub(crate) fn reload_runtime_state() {
    service::clear_cache();
    tree_sitter::reset_registry();
}

pub(crate) fn language_install_state_for_path(
    path: &std::path::Path,
) -> Option<(HighlightLanguage, HighlightInstallState)> {
    detect::detect_language(Some(path), None, "")
        .map(|language| (language, language.install_state()))
}
