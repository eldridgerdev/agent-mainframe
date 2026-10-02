use super::context::{PreparedContext, QuestionContext};
use crate::editor::TextEditor;
use crate::project::AgentKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraftDestination {
    Inline,
    General,
}

#[derive(Debug, Clone)]
pub(crate) struct Turn {
    pub question: String,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub context: QuestionContext,
    pub prepared: Option<PreparedContext>,
}

/// Review-owned UI/history. No database rows or unrelated session state.
#[derive(Debug, Clone)]
pub(crate) struct Questions {
    pub owner: String,
    pub open: bool,
    pub editing: bool,
    pub editor: TextEditor,
    pub turns: Vec<Turn>,
    pub selected: usize,
    pub scroll: usize,
    pub editor_scroll: usize,
    pub started_at: Option<std::time::Instant>,
    pub harness: AgentKind,
    pub request: Option<u64>,
    pub next_request: u64,
    pub error: Option<String>,
    pub current_version: String,
    pub draft: Option<(DraftDestination, TextEditor)>,
}

impl Default for Questions {
    fn default() -> Self {
        Self {
            owner: uuid::Uuid::new_v4().to_string(),
            open: false,
            editing: true,
            editor: TextEditor::new(String::new()),
            turns: Vec::new(),
            selected: 0,
            scroll: 0,
            editor_scroll: 0,
            started_at: None,
            harness: AgentKind::default(),
            request: None,
            next_request: 0,
            error: None,
            current_version: String::new(),
            draft: None,
        }
    }
}
