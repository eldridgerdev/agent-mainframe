use super::*;
use crate::editor::TextEditor;
use std::path::PathBuf;

/// One row in the `Latest Prompt` recall list: either a prompt actually sent
/// to the harness (scanned from its transcript) or one AMF computed to seed a
/// session but couldn't deliver, stashed by
/// [`crate::app::App::stash_lost_prompt`] when the launch that would have
/// carried it failed. Kept as a separate variant rather than folded into
/// [`crate::app::util::PromptEntry`] so an unsent row can carry *why* it's
/// here and be cleared from `unsent_prompts` once picked, without touching
/// the transcript-scan path at all.
#[derive(Clone)]
pub enum LatestPromptItem {
    Sent(crate::app::util::PromptEntry),
    Unsent(crate::db::unsent_prompts::UnsentPrompt),
}

impl LatestPromptItem {
    pub fn text(&self) -> &str {
        match self {
            LatestPromptItem::Sent(entry) => &entry.text,
            LatestPromptItem::Unsent(prompt) => &prompt.body,
        }
    }

    pub fn is_unsent(&self) -> bool {
        matches!(self, LatestPromptItem::Unsent(_))
    }
}

#[derive(Clone)]
pub struct LatestPromptState {
    pub view: ViewState,
    /// Unsent entries for this checkout are prepended ahead of the scanned
    /// transcript history — see `App::poll_latest_prompt_menu_bg`.
    pub prompts: Vec<LatestPromptItem>,
    pub selected: usize,
}

/// A prompt-library row: a template plus where it came from. Phase 1
/// only surfaces `User` templates; the `source` field is ready for the
/// declarative `Global` / `Project` templates added in phase 3.
#[derive(Clone)]
pub struct PromptLibraryEntry {
    pub template: crate::prompt_library::PromptTemplate,
    pub source: crate::prompt_library::PromptSource,
    /// Resolved on-disk location this entry is read from / written to:
    /// the SQLite store for `User`, the relevant `.amf/config.json` for
    /// config sources. Filled in by `rebuild_prompt_library`; `None` when
    /// the scope has no resolvable location (no project context, or the
    /// empty test store path).
    pub source_path: Option<PathBuf>,
}

/// Picker over the merged, source-tagged prompt library. Mirrors the
/// `LatestPrompt` shape (a list with a `selected` index) plus fuzzy
/// filtering and an optional `from_view` to inject back into.
#[derive(Clone)]
pub struct PromptLibraryState {
    pub templates: Vec<PromptLibraryEntry>,
    /// Indices into `templates` matching `query`, best score first.
    pub filtered: Vec<usize>,
    pub query: String,
    pub search_active: bool,
    pub selected: usize,
    /// Set after the first `d` press; a second `d` confirms deletion.
    pub confirm_delete: bool,
    /// Set after `x`; `g` exports to global config, `p` to project.
    pub pending_export: bool,
    pub(crate) file_export: Option<super::prompt_library_files::PromptFileExport>,
    pub(crate) replacement: Option<super::prompt_library_files::PromptFileReplacement>,
    pub from_view: Option<ViewState>,
}

impl PromptLibraryState {
    /// The template currently highlighted in the filtered list.
    pub fn selected_entry(&self) -> Option<&PromptLibraryEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|idx| self.templates.get(*idx))
    }
}

/// Which field of the prompt editor currently has focus. `Tab` cycles
/// Name → Tags → Body (and `Shift+Tab` the reverse). Name and Tags are
/// single-line text fields; Body is the multi-line `TextEditor`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptEditorFocus {
    Name,
    Tags,
    Body,
}

impl PromptEditorFocus {
    /// The next field in the Name → Tags → Body → Name cycle.
    pub fn next(self) -> Self {
        match self {
            PromptEditorFocus::Name => PromptEditorFocus::Tags,
            PromptEditorFocus::Tags => PromptEditorFocus::Body,
            PromptEditorFocus::Body => PromptEditorFocus::Name,
        }
    }

    /// The previous field in the cycle (reverse of `next`).
    pub fn prev(self) -> Self {
        match self {
            PromptEditorFocus::Name => PromptEditorFocus::Body,
            PromptEditorFocus::Tags => PromptEditorFocus::Name,
            PromptEditorFocus::Body => PromptEditorFocus::Tags,
        }
    }
}

/// Create/edit dialog for a user template. `editing_id` is `None` for a
/// new template. `return_to` is where to land after save/cancel — the
/// picker it was opened from, or the underlying `Viewing` mode when
/// saved straight from the compose box.
pub struct PromptEditorState {
    pub editing_id: Option<String>,
    /// Where the template lives; determines which store/file is written on save.
    pub editing_source: crate::prompt_library::PromptSource,
    /// Original template for config-file sources (Project/Global/Worktree),
    /// preserving id/description/placeholders across edits.
    pub original_template: Option<crate::prompt_library::PromptTemplate>,
    pub(crate) file_edit: Option<super::prompt_library_files::PromptFileEdit>,
    pub(crate) file_discard: Option<bool>,
    pub name: String,
    /// Raw comma/space-separated tag input; parsed into the template's
    /// `tags` on save (see `prompt_library::parse_tags`).
    pub tags: String,
    pub focus: PromptEditorFocus,
    pub editor: TextEditor,
    pub return_to: Box<AppMode>,
    /// Where a save will land, for the editor's destination hint: the
    /// SQLite store for `User`, the relevant `.amf/config.json` otherwise.
    /// `None` when unresolvable (no project context, or the test store).
    pub dest_path: Option<PathBuf>,
}

/// Collects a value for each `{{slot}}` in a template before injection.
/// One field is shown at a time (`current` of `placeholders.len()`); each
/// slot's value lives in `values` (seeded with its default) so moving
/// back and forth preserves edits. `from_view` is where the rendered
/// prompt is delivered once every field is filled.
pub struct PlaceholderFillState {
    pub template: crate::prompt_library::PromptTemplate,
    /// Slots to fill, in body order. Built by `resolve_placeholders`.
    pub placeholders: Vec<crate::prompt_library::PromptPlaceholder>,
    /// One entry per placeholder; seeded with defaults, updated on nav.
    pub values: Vec<String>,
    pub current: usize,
    /// Editor for the field currently shown; reseeded from `values` on nav.
    /// Unused while the active slot is a `Select` (the option list drives it).
    pub input: TextEditor,
    /// Highlighted option index for a `Select` slot; ignored otherwise.
    pub select_index: usize,
    /// Whether the user has turned vim on for the fill fields. Persisted on
    /// the state (not the editor) so the choice survives `enter()` rebuilding
    /// `input` when moving between slots. Only applies to multi-line slots.
    pub vim_enabled: bool,
    pub from_view: Option<ViewState>,
}

impl PlaceholderFillState {
    pub fn current_placeholder(&self) -> Option<&crate::prompt_library::PromptPlaceholder> {
        self.placeholders.get(self.current)
    }

    /// Whether the active field accepts newlines (Enter inserts a line break
    /// rather than advancing to the next slot).
    pub fn current_is_multiline(&self) -> bool {
        matches!(
            self.current_placeholder().map(|p| &p.kind),
            Some(crate::prompt_library::PlaceholderKind::MultiLine { .. })
        )
    }

    /// Whether the active slot is a `Select` (choose from a fixed option list).
    pub fn is_select(&self) -> bool {
        matches!(
            self.current_placeholder().map(|p| &p.kind),
            Some(crate::prompt_library::PlaceholderKind::Select { .. })
        )
    }

    /// The options for the active slot, or an empty slice when it isn't a
    /// `Select`.
    pub fn current_options(&self) -> &[String] {
        match self.current_placeholder().map(|p| &p.kind) {
            Some(crate::prompt_library::PlaceholderKind::Select { options }) => options.as_slice(),
            _ => &[],
        }
    }

    /// Move to slot `idx`: reseed the editor from its stored value and point
    /// `select_index` at that value's position in the options (0 otherwise).
    pub fn enter(&mut self, idx: usize) {
        self.current = idx;
        let value = self.values.get(idx).cloned().unwrap_or_default();
        let is_multiline = matches!(
            self.placeholders.get(idx).map(|p| &p.kind),
            Some(crate::prompt_library::PlaceholderKind::MultiLine { .. })
        );
        self.select_index = match self.placeholders.get(idx).map(|p| &p.kind) {
            Some(crate::prompt_library::PlaceholderKind::Select { options }) => {
                options.iter().position(|o| o == &value).unwrap_or(0)
            }
            _ => 0,
        };
        // Vim applies only to multi-line slots; single-line/select slots use
        // Enter to advance, so a plain editor keeps that behaviour intact.
        self.input = if self.vim_enabled && is_multiline {
            TextEditor::with_vim(value)
        } else {
            TextEditor::new(value)
        };
    }

    /// Toggle vim on the active multi-line field, remembering the choice for
    /// later slots. No-op (and reports `false`) on non-multi-line slots, where
    /// vim would hijack Enter's "advance field" behaviour.
    pub fn toggle_input_vim(&mut self) -> bool {
        if !self.current_is_multiline() {
            return false;
        }
        self.input.toggle_vim();
        self.vim_enabled = self.input.vim_mode().is_some();
        true
    }

    /// Record the active slot's value into `values`: the chosen option for a
    /// `Select`, the editor text otherwise.
    pub fn commit_current(&mut self) {
        let value = match self.current_placeholder().map(|p| &p.kind) {
            Some(crate::prompt_library::PlaceholderKind::Select { options }) => {
                options.get(self.select_index).cloned().unwrap_or_default()
            }
            _ => self.input.text().to_string(),
        };
        if let Some(slot) = self.values.get_mut(self.current) {
            *slot = value;
        }
    }

    /// Highlight the next option (wrapping) for a `Select` slot.
    pub fn select_next(&mut self) {
        let len = self.current_options().len();
        if len > 0 {
            self.select_index = (self.select_index + 1) % len;
        }
    }

    /// Highlight the previous option (wrapping) for a `Select` slot.
    pub fn select_prev(&mut self) {
        let len = self.current_options().len();
        if len > 0 {
            self.select_index = self.select_index.checked_sub(1).unwrap_or(len - 1);
        }
    }
}
