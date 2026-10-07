mod detect;
mod model;
mod service;
mod theme;
mod tree_sitter;

pub(crate) use detect::{HighlightInstallState, HighlightLanguage};
pub use theme::style_for_class;

#[cfg(test)]
pub(crate) use model::HighlightedSpan;
pub(crate) use model::{HighlightRequest, HighlightedLine, HighlightedText, SyntaxClass};
pub(crate) use service::{cache_generation, highlight_source, parser_state_for};
pub(crate) use tree_sitter::{StartupValidationLevel, StartupValidationMessage};

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

pub(crate) fn validate_startup_parsers() -> Vec<StartupValidationMessage> {
    tree_sitter::validate_startup_parsers()
}

pub(crate) fn language_install_state_for_path(
    path: &std::path::Path,
) -> Option<(HighlightLanguage, HighlightInstallState)> {
    detect::detect_language(Some(path), None, "")
        .map(|language| (language, language.install_state()))
}

/// The language and parser state highlighting this source would use: the same
/// path/shebang detection [`highlight_source`] applies, so a caller can say
/// *why* a file came back plain (unknown language, parser not installed).
pub(crate) fn language_for_source(
    path: Option<&std::path::Path>,
    source: &str,
) -> Option<(HighlightLanguage, HighlightInstallState)> {
    detect::detect_language(path, None, source).map(|language| (language, language.install_state()))
}

/// Whether an installed parser loads; see [`tree_sitter::parser_loads`].
pub(crate) fn parser_loads(language: HighlightLanguage) -> bool {
    tree_sitter::parser_loads(language)
}
