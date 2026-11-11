//! Reference notes an agent attaches to a Terminal Session.
//!
//! When an agent asks the user to run something in another terminal, it can
//! leave the commands and context the user needs as notes. Per feature they
//! live in two generated files under `<workdir>/.amf/` (gitignored with the
//! rest of `.amf/`):
//!
//! - `terminal-notes.json`, the source of truth: one entry per terminal
//!   session, keyed by session id.
//! - `terminal-notes.md`, rendered from it on every change, which is what the
//!   markdown viewer opens. Keeping bookkeeping out of the markdown means the
//!   user reads clean text, and a note body may contain any markdown.
//!
//! One file for the whole feature means a single viewer shows every
//! terminal's notes, and the sidebar needs one `stat` to know whether to show
//! its section at all.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::*;

const MARKDOWN_FILE: &str = "terminal-notes.md";
const DATA_FILE: &str = "terminal-notes.json";
/// Longest note AMF will store for one terminal.
pub(crate) const MAX_NOTES_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NoteEntry {
    pub id: String,
    pub label: String,
    pub body: String,
}

/// The rendered markdown the viewer opens.
pub(crate) fn notes_path(workdir: &Path) -> PathBuf {
    workdir.join(".amf").join(MARKDOWN_FILE)
}

fn data_path(workdir: &Path) -> PathBuf {
    workdir.join(".amf").join(DATA_FILE)
}

/// A missing or unreadable file is "no notes", never an error.
pub(crate) fn load(workdir: &Path) -> Vec<NoteEntry> {
    std::fs::read_to_string(data_path(workdir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn render_markdown(entries: &[NoteEntry]) -> String {
    let mut out = String::from("# Terminal notes\n");
    for entry in entries {
        let label = entry.label.replace(['\n', '\r'], " ");
        out.push_str(&format!("\n---\n\n## {label}\n\n{}\n", entry.body.trim()));
    }
    out
}

/// Replace `id`'s entry in place, or append one. Blank `notes` removes it.
pub(crate) fn upsert(
    mut entries: Vec<NoteEntry>,
    id: &str,
    label: &str,
    notes: &str,
) -> Vec<NoteEntry> {
    let body = notes.trim().to_string();
    match entries.iter().position(|entry| entry.id == id) {
        Some(at) if body.is_empty() => {
            entries.remove(at);
        }
        Some(at) => {
            entries[at].label = label.to_string();
            entries[at].body = body;
        }
        None if body.is_empty() => {}
        None => entries.push(NoteEntry {
            id: id.to_string(),
            label: label.to_string(),
            body,
        }),
    }
    entries
}

/// Write `id`'s notes into the feature's files, creating `.amf/` as needed.
/// Blank `notes` clears them. Both files go when the last entry does.
pub(crate) fn set_notes(workdir: &Path, id: &str, label: &str, notes: &str) -> Result<()> {
    if notes.len() > MAX_NOTES_BYTES {
        anyhow::bail!("notes are too long (limit {} KiB)", MAX_NOTES_BYTES / 1024);
    }
    save(workdir, &upsert(load(workdir), id, label, notes))
}

/// Drop `id`'s notes, if any. Nothing to drop is not an error.
pub(crate) fn clear_notes(workdir: &Path, id: &str) -> Result<()> {
    let entries = load(workdir);
    if !entries.iter().any(|entry| entry.id == id) {
        return Ok(());
    }
    save(workdir, &upsert(entries, id, "", ""))
}

fn save(workdir: &Path, entries: &[NoteEntry]) -> Result<()> {
    let (data, markdown) = (data_path(workdir), notes_path(workdir));
    if entries.is_empty() {
        for path in [&data, &markdown] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        return Ok(());
    }
    // Creates `.amf/` and its README: the directory AMF already ignores.
    crate::extension::generated_amf_subdir(workdir, "");
    std::fs::write(&data, serde_json::to_string_pretty(entries)?)?;
    std::fs::write(&markdown, render_markdown(entries))?;
    Ok(())
}

/// Sidebar text for a feature: the terminals that have notes, one per line.
/// `None` when there are none, so the section does not appear.
///
/// One `stat` decides, and the data is read only when it exists, so a
/// feature without notes costs nothing per frame.
pub(crate) fn sidebar_text(workdir: &Path) -> Option<String> {
    std::fs::metadata(data_path(workdir)).ok()?;
    let entries = load(workdir);
    if entries.is_empty() {
        return None;
    }
    Some(
        entries
            .iter()
            .map(|entry| {
                if entry.label.trim().is_empty() {
                    "Terminal".to_string()
                } else {
                    entry.label.replace(['\n', '\r'], " ")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

impl App {
    /// Open the feature's terminal notes in the markdown viewer (`leader u`).
    pub fn open_terminal_notes_from_view(&mut self) -> Result<()> {
        let view = match &self.mode {
            AppMode::Viewing(view) => view.clone(),
            _ => return Ok(()),
        };
        let Some(workdir) = self
            .store
            .projects
            .iter()
            .flat_map(|p| &p.features)
            .find(|f| f.tmux_session == view.session)
            .map(|f| f.workdir.clone())
        else {
            self.push_toast_warning("Could not resolve the current feature");
            return Ok(());
        };
        if sidebar_text(&workdir).is_none() {
            self.push_toast_warning(
                "No terminal notes yet. An agent adds them when it opens a terminal for you.",
            );
            return Ok(());
        }
        self.open_markdown_viewer_path(notes_path(&workdir), workdir, None, view, None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_in_place_and_keeps_order() {
        let entries = upsert(vec![], "a", "dev server", "Run `npm run dev`");
        let entries = upsert(
            entries,
            "b",
            "tests",
            "## Heading in a note\n\n`cargo test`",
        );
        let entries = upsert(entries, "a", "dev server", "Run `npm start`");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "a");
        assert_eq!(entries[0].body, "Run `npm start`");
        assert_eq!(entries[1].body, "## Heading in a note\n\n`cargo test`");
    }

    #[test]
    fn blank_notes_remove_the_entry_and_unknown_ids_are_ignored() {
        let entries = upsert(vec![], "a", "x", "note");
        assert!(upsert(entries.clone(), "a", "x", "  \n").is_empty());
        assert_eq!(upsert(entries, "zzz", "y", "").len(), 1);
    }

    #[test]
    fn rendered_markdown_is_clean_text_with_no_bookkeeping() {
        let entries = upsert(
            upsert(vec![], "id-1", "dev server", "run it"),
            "id-2",
            "tests",
            "test it",
        );
        let md = render_markdown(&entries);
        assert!(md.starts_with("# Terminal notes\n"));
        assert!(md.contains("## dev server\n\nrun it\n"));
        assert!(md.contains("## tests\n\ntest it\n"));
        assert!(!md.contains("id-1") && !md.contains("<!--"));
    }

    #[test]
    fn files_are_written_listed_and_removed_with_the_last_note() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(sidebar_text(dir.path()).is_none());
        set_notes(dir.path(), "a", "dev server", "run it").unwrap();
        set_notes(dir.path(), "b", "tests", "test it").unwrap();
        assert_eq!(sidebar_text(dir.path()).unwrap(), "dev server\ntests");
        assert!(
            std::fs::read_to_string(notes_path(dir.path()))
                .unwrap()
                .contains("test it")
        );
        clear_notes(dir.path(), "a").unwrap();
        assert_eq!(sidebar_text(dir.path()).unwrap(), "tests");
        clear_notes(dir.path(), "b").unwrap();
        assert!(!notes_path(dir.path()).exists() && !data_path(dir.path()).exists());
        clear_notes(dir.path(), "b").unwrap();
        assert!(set_notes(dir.path(), "a", "x", &"n".repeat(MAX_NOTES_BYTES + 1)).is_err());
    }
}
