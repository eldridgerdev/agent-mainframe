use super::*;
use crate::editor::TextEditor;

// ── Prompt-override manager (Editable Headless Prompts) ──────────────────

/// Which scope an override is being saved to. `Feature` is only offered when
/// a feature is in context and a database is present.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptOverrideScope {
    Feature,
    Project,
    Global,
}

impl PromptOverrideScope {
    pub fn label(self) -> &'static str {
        match self {
            PromptOverrideScope::Feature => "This feature",
            PromptOverrideScope::Project => "This project (amf.json)",
            PromptOverrideScope::Global => "Global (all projects)",
        }
    }
}

/// The step the editor is on: typing the template, then picking a scope, then
/// picking a harness variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptOverrideStep {
    Editing,
    ScopePicker,
    HarnessPicker,
}

/// One list row: a registry prompt and where its effective template (for the
/// shared/no-harness view) currently comes from.
#[derive(Clone)]
pub struct PromptOverrideRow {
    pub id: crate::prompts::PromptId,
    pub source: crate::prompts::PromptSource,
    /// True when the row has an override at that scope (any harness).
    pub has_feature: bool,
    pub has_project: bool,
    pub has_global: bool,
}

impl PromptOverrideRow {
    /// True when any scope overrides this prompt (for any harness).
    pub fn is_overridden(&self) -> bool {
        self.source.is_override() || self.has_feature || self.has_project || self.has_global
    }
}

/// The editor sub-state, open for `rows[row]`.
pub struct PromptOverrideEditState {
    pub row: usize,
    pub editor: TextEditor,
    pub step: PromptOverrideStep,
    /// Scopes offered, narrowest first. `Feature` is present only with a
    /// feature in context and a DB.
    pub scopes: Vec<PromptOverrideScope>,
    pub scope_index: usize,
    /// 0 = shared; 1..=4 = `AgentKind::ALL[i-1]`.
    pub harness_index: usize,
}

impl PromptOverrideEditState {
    pub fn scope(&self) -> PromptOverrideScope {
        self.scopes
            .get(self.scope_index)
            .copied()
            .unwrap_or(PromptOverrideScope::Global)
    }

    /// `None` = shared across harnesses; `Some` = one specific harness.
    pub fn harness(&self) -> Option<crate::project::AgentKind> {
        self.harness_index
            .checked_sub(1)
            .and_then(|i| crate::project::AgentKind::ALL.get(i).cloned())
    }
}

/// The prompt-override manager overlay: a list of every registry prompt with
/// its effective source, an inline template editor, and scope / harness
/// pickers on save.
pub struct PromptOverridesState {
    /// One row per `crate::prompts::PromptId::ALL`, in that order.
    pub rows: Vec<PromptOverrideRow>,
    pub selected: usize,
    pub scroll: usize,
    pub edit: Option<PromptOverrideEditState>,
    pub help_open: bool,
    /// A second `d` in the list confirms clearing the selected row's override.
    pub confirm_clear: bool,
    pub from_view: Option<ViewState>,
    /// List every prompt (`true`) or only the overridden ones (`false`, the
    /// default). `selected` always indexes `rows`; `scroll` indexes the
    /// *visible* list.
    pub show_all: bool,
}

impl PromptOverridesState {
    pub fn selected_row(&self) -> Option<&PromptOverrideRow> {
        self.rows.get(self.selected)
    }

    /// Whether row `i` appears in the list: always when `show_all`, else only
    /// when some scope overrides it.
    pub fn is_visible(&self, i: usize) -> bool {
        self.show_all
            || self
                .rows
                .get(i)
                .is_some_and(PromptOverrideRow::is_overridden)
    }

    /// Indices into `rows` of the rows the list currently shows.
    pub fn visible_indices(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|i| self.is_visible(*i))
            .collect()
    }
}

pub struct HelpState {
    pub from_view: Option<ViewState>,
    pub scroll_offset: usize,
    /// Filter typed after `/`; rows that don't match are hidden.
    pub query: String,
    /// `/` was pressed and keys are being typed into `query`.
    pub searching: bool,
}

impl HelpState {
    pub fn new(from_view: Option<ViewState>) -> Self {
        Self {
            from_view,
            scroll_offset: 0,
            query: String::new(),
            searching: false,
        }
    }
}

/// A search-as-you-type picker over the workspace's agent skills, launched
/// from a prompt-editing surface (the prompt editor body or a text fill
/// field). Selecting an entry inserts its `/skill-name` invocation at the
/// editor cursor; `return_to` holds the editing mode to restore afterwards.
pub struct SkillPickerState {
    /// All available skills (global + project), name-sorted.
    pub skills: Vec<ComposeCommandEntry>,
    /// Indices into `skills` matching the current query, best match first.
    pub filtered: Vec<usize>,
    pub query: String,
    pub selected: usize,
    /// The editing mode to return to on select/cancel — `PromptEditor` or
    /// `PlaceholderFill`. Boxed because `AppMode` is large.
    pub return_to: Box<AppMode>,
}

impl SkillPickerState {
    /// The currently highlighted skill, if any survive the filter.
    pub fn selected_skill(&self) -> Option<&ComposeCommandEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|idx| self.skills.get(*idx))
    }
}
