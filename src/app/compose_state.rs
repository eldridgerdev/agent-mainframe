use super::*;
use crate::editor::TextEditor;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeCommandSource {
    BuiltIn,
    Global,
    Project,
    Skill,
}

impl ComposeCommandSource {
    pub fn label(self) -> &'static str {
        match self {
            ComposeCommandSource::BuiltIn => "Built-in",
            ComposeCommandSource::Global => "Global",
            ComposeCommandSource::Project => "Project",
            ComposeCommandSource::Skill => "Skill",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComposeCommandEntry {
    /// Command name without the leading slash (e.g. "compact").
    pub name: String,
    pub description: String,
    pub source: ComposeCommandSource,
    /// True when the command opens a CC-owned interactive dialog;
    /// submitting it drops the session into direct (passthrough) mode.
    pub interactive: bool,
}

/// An image pasted into the compose box, shown as a `[Image N]`
/// placeholder in the editor and delivered to Claude Code via the
/// clipboard at submit time.
#[derive(Clone)]
pub struct ComposeImage {
    pub placeholder: String,
    pub data: Vec<u8>,
    pub mime: String,
}

/// Unsent compose content saved when the box closes without sending.
#[derive(Clone, Default)]
pub struct ComposeDraft {
    pub text: String,
    pub images: Vec<ComposeImage>,
}

#[derive(Clone)]
pub struct ComposeState {
    pub view: ViewState,
    pub workdir: PathBuf,
    pub editor: TextEditor,
    pub scroll_offset: usize,
    pub sync_scroll_to_cursor: bool,
    /// Full command catalog built when the compose box opens.
    pub catalog: Vec<ComposeCommandEntry>,
    /// Catalog indices currently matching the typed /prefix.
    pub suggestions: Vec<usize>,
    pub suggestion_index: usize,
    /// Pasted images, in placeholder order.
    pub images: Vec<ComposeImage>,
    /// Background clipboard read currently feeding this compose box.
    pub clipboard_paste_id: Option<u64>,
    /// The compose buffer is being delivered to the harness in a
    /// background worker. This is used for the slower WSL image path.
    pub submit_in_progress: bool,
}

impl ComposeState {
    pub fn new(
        view: ViewState,
        workdir: PathBuf,
        text: String,
        catalog: Vec<ComposeCommandEntry>,
    ) -> Self {
        let mut state = Self {
            view,
            workdir,
            editor: TextEditor::new(text),
            scroll_offset: 0,
            sync_scroll_to_cursor: true,
            catalog,
            suggestions: Vec::new(),
            suggestion_index: 0,
            images: Vec::new(),
            clipboard_paste_id: None,
            submit_in_progress: false,
        };
        state.refresh_suggestions();
        state
    }

    /// Register a pasted image and return the placeholder to insert
    /// into the editor.
    pub fn add_image(&mut self, data: Vec<u8>, mime: String) -> String {
        let placeholder = format!("[Image {}]", self.images.len() + 1);
        self.images.push(ComposeImage {
            placeholder: placeholder.clone(),
            data,
            mime,
        });
        placeholder
    }

    pub fn request_cursor_scroll(&mut self) {
        self.sync_scroll_to_cursor = true;
    }

    pub fn paste_in_progress(&self) -> bool {
        self.clipboard_paste_id.is_some() || self.submit_in_progress
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.sync_scroll_to_cursor = false;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
        self.sync_scroll_to_cursor = false;
    }

    pub fn clear_prompt(&mut self) -> bool {
        let cleared = self.editor.clear().text_changed || !self.images.is_empty();
        self.images.clear();
        self.scroll_offset = 0;
        self.sync_scroll_to_cursor = false;
        self.refresh_suggestions();
        cleared
    }

    /// The /command token being typed, if the buffer is a single line
    /// starting with '/' and no arguments have been typed yet.
    pub fn pending_command_prefix(&self) -> Option<&str> {
        let text = self.editor.text();
        let rest = text.strip_prefix('/')?;
        if rest.contains('\n') || rest.contains(' ') {
            return None;
        }
        Some(rest)
    }

    /// True when the buffer holds a single-line /command (with or
    /// without arguments) that should be delivered as keystrokes.
    pub fn is_slash_command(&self) -> bool {
        let text = self.editor.text().trim();
        text.starts_with('/') && !text.contains('\n')
    }

    pub fn refresh_suggestions(&mut self) {
        let previously_selected = self.suggestions.get(self.suggestion_index).copied();

        match self.pending_command_prefix() {
            Some(prefix) => {
                // Fuzzy-rank the catalog so a query like "commit"
                // matches namespaced commands such as "stn:commit".
                let mut scored: Vec<(i32, usize)> = self
                    .catalog
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, entry)| {
                        crate::app::compose::fuzzy_score(prefix, &entry.name)
                            .map(|score| (score, idx))
                    })
                    .collect();
                // Highest score first; ties keep catalog order, which
                // preserves the built-in/global/project/skill grouping.
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                self.suggestions = scored.into_iter().map(|(_, idx)| idx).collect();
            }
            None => self.suggestions.clear(),
        }

        self.suggestion_index = previously_selected
            .and_then(|catalog_idx| self.suggestions.iter().position(|idx| *idx == catalog_idx))
            .unwrap_or(0);
    }

    pub fn selected_suggestion(&self) -> Option<&ComposeCommandEntry> {
        self.suggestions
            .get(self.suggestion_index)
            .and_then(|idx| self.catalog.get(*idx))
    }

    pub fn select_next_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        self.suggestion_index = (self.suggestion_index + 1) % self.suggestions.len();
    }

    pub fn select_prev_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        self.suggestion_index = self
            .suggestion_index
            .checked_sub(1)
            .unwrap_or(self.suggestions.len() - 1);
    }

    /// Replace the typed /prefix with the selected suggestion's name.
    /// Returns true if a completion was applied.
    pub fn complete_selected_suggestion(&mut self) -> bool {
        let Some(entry) = self.selected_suggestion() else {
            return false;
        };
        let completed = format!("/{}", entry.name);
        if self.editor.text() == completed {
            return false;
        }
        self.editor = TextEditor::new(completed);
        self.refresh_suggestions();
        self.request_cursor_scroll();
        true
    }

    /// The command catalog entry matching the buffer exactly, if any.
    pub fn exact_command_match(&self) -> Option<&ComposeCommandEntry> {
        let text = self.editor.text().trim();
        let rest = text.strip_prefix('/')?;
        let name = rest.split_whitespace().next()?;
        self.catalog
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
    }
}
