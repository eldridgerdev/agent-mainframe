//! Full catalog over the same installer/coordinator as per-file syntax badges.
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    HighlightInstallState, HighlightLanguage, InstallState, SyntaxInstallView,
    SyntaxOperationAction, highlight, install_parser, install_state, poisoned, start_operation,
};
use crate::gui_contract::{GuiError, GuiResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParserState {
    Available,
    Installed,
    Broken,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyntaxLanguageRow {
    pub key: String,
    pub title: String,
    pub state: ParserState,
    pub revision: String,
}

#[derive(Debug, Serialize)]
pub struct SyntaxLanguagesView {
    pub languages: Vec<SyntaxLanguageRow>,
    pub operation: SyntaxInstallView,
}

#[derive(Debug, Deserialize)]
pub struct SyntaxLanguageRequest {
    pub language: String,
    pub action: SyntaxOperationAction,
    pub revision: String,
}

fn stamp_metadata(path: &Path, digest: &mut Sha256, recursive: bool) -> anyhow::Result<()> {
    digest.update(path.as_os_str().as_encoded_bytes());
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            digest.update(b"missing");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest.update(metadata.dev().to_le_bytes());
        digest.update(metadata.ino().to_le_bytes());
    }
    digest.update(format!(
        "{:?}:{}:{:?}",
        metadata.file_type(),
        metadata.len(),
        metadata.modified()?
    ));
    if metadata.is_symlink() {
        digest.update(std::fs::read_link(path)?.as_os_str().as_encoded_bytes());
    } else if metadata.is_dir() && recursive {
        let mut children = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            stamp_metadata(&child, digest, true)?;
        }
    }
    Ok(())
}

fn stamp_contents(path: &Path, digest: &mut Sha256) -> anyhow::Result<()> {
    stamp_metadata(path, digest, false)?;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(())
}

fn disk_revision(
    language: HighlightLanguage,
    state: ParserState,
    source: &Path,
    library: &Path,
) -> anyhow::Result<String> {
    let mut digest = Sha256::new();
    digest.update(language.package_key().as_bytes());
    digest.update(serde_json::to_vec(&state)?);
    let mut root = source
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("Parser source has no managed root"))?;
    // Bind the destination even before it exists, including an ancestor that
    // could otherwise be replaced/repointed between list and confirmation.
    loop {
        stamp_metadata(root, &mut digest, false)?;
        if root.exists() {
            digest.update(root.canonicalize()?.as_os_str().as_encoded_bytes());
            break;
        }
        root = root
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Parser root has no existing ancestor"))?;
    }
    // The complete managed source tree's identities/times catch external edits;
    // hash loaded library/query bytes as well, including same-size replacement.
    stamp_metadata(source, &mut digest, true)?;
    stamp_contents(library, &mut digest)?;
    for grammar in language.package_spec().grammars {
        for query in [
            grammar.highlights_query,
            grammar.injections_query,
            grammar.locals_query,
        ] {
            if !query.is_empty() {
                stamp_contents(&source.join(query), &mut digest)?;
            }
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn language_row(language: HighlightLanguage) -> GuiResult<SyntaxLanguageRow> {
    let state = match language.install_state() {
        HighlightInstallState::Available => ParserState::Available,
        HighlightInstallState::Broken => ParserState::Broken,
        HighlightInstallState::Installed if highlight::parser_loads(language) => {
            ParserState::Installed
        }
        HighlightInstallState::Installed => ParserState::Broken,
    };
    let revision = disk_revision(
        language,
        state,
        &language.source_dir(),
        &language.library_path(),
    )?;
    Ok(SyntaxLanguageRow {
        key: language.package_key().into(),
        title: language.picker_title().into(),
        state,
        revision,
    })
}

pub fn languages_list(refresh: bool) -> GuiResult<SyntaxLanguagesView> {
    read_languages(
        install_state(),
        || {
            if refresh {
                highlight::reload_runtime_state();
            }
        },
        language_row,
    )
}

fn read_languages<F, R>(
    state: &InstallState,
    before_read: F,
    read_row: R,
) -> GuiResult<SyntaxLanguagesView>
where
    F: FnOnce(),
    R: FnMut(HighlightLanguage) -> GuiResult<SyntaxLanguageRow>,
{
    let operation = state.lock().map_err(|_| poisoned())?;
    // Do not try to load a library while the shared worker is replacing it.
    if operation.running {
        return Ok(SyntaxLanguagesView {
            languages: Vec::new(),
            operation: operation.clone(),
        });
    }
    before_read();
    let languages = HighlightLanguage::ALL
        .into_iter()
        .map(read_row)
        .collect::<GuiResult<_>>()?;
    Ok(SyntaxLanguagesView {
        languages,
        operation: operation.clone(),
    })
}

fn apply_with<R, I>(
    state: &InstallState,
    request: SyntaxLanguageRequest,
    read_row: R,
    operation: I,
) -> GuiResult<SyntaxInstallView>
where
    R: FnOnce(HighlightLanguage) -> GuiResult<SyntaxLanguageRow>,
    I: FnOnce(HighlightLanguage, Box<dyn FnMut(String) + Send>) -> anyhow::Result<String>
        + Send
        + 'static,
{
    start_operation(
        state,
        &request.language,
        request.action,
        |language| {
            let fresh = read_row(language)?;
            if fresh.revision != request.revision {
                return Err(GuiError::conflict(
                    "Parser files changed. Refresh languages and confirm again.",
                ));
            }
            let valid = match request.action {
                SyntaxOperationAction::Install => fresh.state == ParserState::Available,
                SyntaxOperationAction::Repair | SyntaxOperationAction::Uninstall => {
                    fresh.state != ParserState::Available
                }
            };
            if !valid {
                return Err(GuiError::conflict(
                    "Parser state changed. Refresh languages and choose an available action.",
                ));
            }
            Ok(())
        },
        operation,
    )
}

pub fn languages_apply(request: SyntaxLanguageRequest) -> GuiResult<SyntaxInstallView> {
    let action = request.action;
    apply_with(
        install_state(),
        request,
        |language| {
            // A TUI or another GUI may have changed files since the list loaded.
            highlight::reload_runtime_state();
            language_row(language)
        },
        move |language, progress| match action {
            SyntaxOperationAction::Install | SyntaxOperationAction::Repair => {
                install_parser(language, progress)
            }
            SyntaxOperationAction::Uninstall => highlight::uninstall_language(language, progress),
        },
    )
}

#[cfg(test)]
mod tests;
