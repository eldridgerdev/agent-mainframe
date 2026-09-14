use crate::editor::TextEditor;
use crate::project::AgentKind;
use std::path::PathBuf;
/// Which files Learning Mode lists.
// Learning Mode's overlay lands in the plan's Epics 2-5
// (`docs/backlog/learning-mode-plan.md`), so this state is written before
// anything reads it. The `dead_code` allows through the end of
// `LearningViewState` come off in Epic 6.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseScope {
    /// Every file in the project's working tree (git-tracked plus untracked
    /// files git doesn't ignore).
    RepoTree,
    /// Only the files changed on the feature's branch.
    BranchChanges,
}

#[allow(dead_code)]
impl BrowseScope {
    /// Short header label.
    pub fn label(self) -> &'static str {
        match self {
            BrowseScope::RepoTree => "Repo tree",
            BrowseScope::BranchChanges => "Branch changes",
        }
    }

    /// Spelled-out description — the header says what the scope *is* rather
    /// than relying on the user knowing AMF's vocabulary.
    pub fn description(self) -> &'static str {
        match self {
            BrowseScope::RepoTree => "all files in this project",
            BrowseScope::BranchChanges => "files changed on this branch",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            BrowseScope::RepoTree => BrowseScope::BranchChanges,
            BrowseScope::BranchChanges => BrowseScope::RepoTree,
        }
    }
}

/// What a Learning Mode question is asked *about*. Persisted as an
/// `anchor_kind` string plus an optional line range (see [`LearningAnchor::kind_str`]
/// and [`LearningAnchor::from_parts`]).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAnchor {
    /// The repository as a whole — used by the orientation ("give me a tour")
    /// question, which has no file to point at.
    Project,
    /// The whole of the currently loaded file.
    File,
    /// A hunk of the current file's diff, by index into `DiffFile::hunks`.
    /// Only reachable in [`BrowseScope::BranchChanges`] — repo-tree browsing
    /// has no diff, so a hunk has nothing to mean there.
    Hunk { index: usize },
    /// An inclusive 1-based line range in the current file.
    Lines { start: usize, end: usize },
}

#[allow(dead_code)]
impl LearningAnchor {
    /// Stable string stored in `learning_qa.anchor_kind`.
    pub fn kind_str(self) -> &'static str {
        match self {
            LearningAnchor::Project => "project",
            LearningAnchor::File => "file",
            LearningAnchor::Hunk { .. } => "hunk",
            LearningAnchor::Lines { .. } => "lines",
        }
    }

    /// The persisted line range: `(line_start, line_end)`, both `None` for
    /// anchors that cover no specific lines.
    pub fn line_range(self) -> (Option<usize>, Option<usize>) {
        match self {
            LearningAnchor::Project | LearningAnchor::File => (None, None),
            LearningAnchor::Hunk { index } => (Some(index), None),
            LearningAnchor::Lines { start, end } => (Some(start), Some(end)),
        }
    }

    /// Rebuild an anchor from its persisted parts. Unknown kinds and
    /// range-less `lines` rows fall back to [`LearningAnchor::File`] rather
    /// than failing the load — a slightly coarse anchor beats a lost note.
    pub fn from_parts(kind: &str, start: Option<usize>, end: Option<usize>) -> Self {
        match kind {
            "project" => LearningAnchor::Project,
            "hunk" => match start {
                Some(index) => LearningAnchor::Hunk { index },
                None => LearningAnchor::File,
            },
            "lines" => match (start, end) {
                (Some(start), Some(end)) => LearningAnchor::Lines { start, end },
                (Some(start), None) => LearningAnchor::Lines { start, end: start },
                _ => LearningAnchor::File,
            },
            _ => LearningAnchor::File,
        }
    }

    /// The 1-based inclusive line range this anchor actually names, for prose
    /// that quotes it back. Unlike [`line_range`](Self::line_range) — which is
    /// the persistence shape and reuses `line_start` to hold a hunk index —
    /// this is `None` for every anchor that does not cover specific lines.
    pub fn line_range_for_display(self) -> Option<(usize, usize)> {
        match self {
            LearningAnchor::Lines { start, end } => Some((start, end)),
            _ => None,
        }
    }

    /// Plain-words description echoed above the question input, e.g.
    /// `lines 40-58 of src/app/learning.rs`.
    pub fn describe(self, path: Option<&str>) -> String {
        let path = path.unwrap_or("this file");
        match self {
            LearningAnchor::Project => "this whole project".to_string(),
            LearningAnchor::File => format!("all of {path}"),
            LearningAnchor::Hunk { index } => format!("change #{} in {path}", index + 1),
            LearningAnchor::Lines { start, end } if start == end => {
                format!("line {start} of {path}")
            }
            LearningAnchor::Lines { start, end } => format!("lines {start}-{end} of {path}"),
        }
    }
}

/// What became of a stored Q&A anchor when it was checked against the file as
/// it stands now.
///
/// Computed when the history loads and **never persisted**. The row's
/// `selection_text` is the evidence, so the verdict can always be re-derived,
/// and the stored `line_start`/`line_end` stay what they have always been: the
/// historical fact of where the question was asked. Overwriting them would
/// trade a recoverable answer for an unrecoverable one.
///
/// Absence of a verdict means "still where it was stored, as far as we can
/// tell" — which is also what a row with nothing to check against reports, so
/// the common case costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAnchorDrift {
    /// The code moved and was found again, at this 1-based inclusive range.
    Reanchored { start: usize, end: usize },
    /// The code the question was asked about can no longer be pointed at.
    Lost(LearningAnchorLoss),
}

/// Why an anchor was given up on. Each reads differently to the user: a
/// deleted file is not the same event as code that was rewritten, and neither
/// is the same as code that now appears in several places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAnchorLoss {
    /// The file itself is no longer in the working directory.
    FileGone,
    /// The file is there, but the selected text is not in it any more.
    NotFound,
    /// The selected text now appears more than once, so there is no honest
    /// way to say which copy the question was about.
    Ambiguous,
}

#[allow(dead_code)]
impl LearningAnchorDrift {
    /// Compact marker for the Q&A history row.
    pub fn marker(self) -> &'static str {
        match self {
            LearningAnchorDrift::Reanchored { .. } => "⚠ moved",
            LearningAnchorDrift::Lost(_) => "⚠ anchor lost",
        }
    }

    /// Whether this is a loss rather than a relocation — the two are coloured
    /// differently, because one of them still points at the right code.
    pub fn is_lost(self) -> bool {
        matches!(self, LearningAnchorDrift::Lost(_))
    }

    /// Full sentence for the answer pane, given the range the row was stored
    /// with. Says what happened *and* that the question and answer are intact,
    /// since a newcomer's reading of "anchor lost" is otherwise "this entry is
    /// broken".
    pub fn describe(self, stored: Option<(usize, usize)>) -> String {
        let was = match stored {
            Some((start, end)) if start == end => format!("line {start}"),
            Some((start, end)) => format!("lines {start}-{end}"),
            None => "this file".to_string(),
        };
        match self {
            LearningAnchorDrift::Reanchored { start, end } if start == end => {
                format!("The code has moved since this was asked: it was {was}, it is now line {start}.")
            }
            LearningAnchorDrift::Reanchored { start, end } => {
                format!(
                    "The code has moved since this was asked: it was {was}, it is now lines {start}-{end}."
                )
            }
            LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone) => {
                "This file is no longer in the project, so there is nothing left to point at. The question and answer below are unchanged.".to_string()
            }
            LearningAnchorDrift::Lost(LearningAnchorLoss::NotFound) => {
                format!(
                    "The code this was asked about is no longer in the file, so {was} now shows something else. The question and answer below are unchanged."
                )
            }
            LearningAnchorDrift::Lost(LearningAnchorLoss::Ambiguous) => {
                "This code now appears in more than one place in the file, so there is no way to say which copy the question was about. The question and answer below are unchanged.".to_string()
            }
        }
    }
}

/// What the user is asking for. Chosen at ask time and re-labelable
/// afterwards; it shapes the prompt framing and which follow-up action the UI
/// offers first, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningQaIntent {
    /// Teach me what this does. No change is proposed; the answer lives on as
    /// an anchored note.
    Explain,
    /// Propose a concrete change.
    Action,
}

#[allow(dead_code)]
impl LearningQaIntent {
    /// Stable string stored in `learning_qa.intent`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningQaIntent::Explain => "explain",
            LearningQaIntent::Action => "action",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "action" => LearningQaIntent::Action,
            _ => LearningQaIntent::Explain,
        }
    }

    /// The label the ask keys carry in the UI.
    pub fn label(self) -> &'static str {
        match self {
            LearningQaIntent::Explain => "Explain this to me",
            LearningQaIntent::Action => "Ask for a change",
        }
    }

    /// Compact marker + word shown on a Q&A row.
    pub fn marker(self) -> &'static str {
        match self {
            LearningQaIntent::Explain => "? explain",
            LearningQaIntent::Action => "! change",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            LearningQaIntent::Explain => LearningQaIntent::Action,
            LearningQaIntent::Action => LearningQaIntent::Explain,
        }
    }
}

/// How much the answer should assume. A per-session setting, not a
/// per-question one; it changes prompt wording only — never tools, model, or
/// which files are visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningLevel {
    /// Default: assume no prior knowledge of this codebase, define jargon,
    /// end with a "Where to look next" pointer.
    Newcomer,
    /// Denser answers for a user who has outgrown the newcomer framing.
    Familiar,
}

#[allow(dead_code)]
impl LearningLevel {
    /// Stable string stored in `learning_sessions.level` / `learning_qa.level`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningLevel::Newcomer => "newcomer",
            LearningLevel::Familiar => "familiar",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "familiar" => LearningLevel::Familiar,
            _ => LearningLevel::Newcomer,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            LearningLevel::Newcomer => LearningLevel::Familiar,
            LearningLevel::Familiar => LearningLevel::Newcomer,
        }
    }
}

/// Whether an answer came from the fast no-tools pass or the slower pass that
/// lets the agent read the rest of the repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningRunMode {
    /// `HeadlessRunner::run(.., restricted = true)` — answered from the
    /// prompt's own context only.
    NoTools,
    /// `HeadlessRunner::run_read_only` — the agent may read the repository.
    DeepDive,
}

#[allow(dead_code)]
impl LearningRunMode {
    /// Stable string stored in `learning_qa.run_mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningRunMode::NoTools => "no_tools",
            LearningRunMode::DeepDive => "deep_dive",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "deep_dive" => LearningRunMode::DeepDive,
            _ => LearningRunMode::NoTools,
        }
    }

    /// The mode `harness` can actually deliver.
    ///
    /// Codex has no no-tools headless invocation: `codex exec` is always an
    /// ephemeral read-only sandbox that can read the whole repository, and
    /// `HeadlessRunner::run` ignores `restricted` for it. Asking for
    /// [`NoTools`](Self::NoTools) there would run a repo-reading agent while
    /// the row claimed "this file only", so the request is downgraded to
    /// [`DeepDive`](Self::DeepDive) before it is recorded or run — the label,
    /// the stored row, and the command then all say the same thing.
    pub fn effective_for(self, harness: &AgentKind) -> Self {
        match (self, harness) {
            (LearningRunMode::NoTools, AgentKind::Codex) => LearningRunMode::DeepDive,
            _ => self,
        }
    }

    /// What the mode does, in the user's terms rather than AMF's.
    pub fn description(self) -> &'static str {
        match self {
            LearningRunMode::NoTools => "this file only",
            LearningRunMode::DeepDive => "read the repo",
        }
    }
}

/// Lifecycle of one queued question. Rendered as a full word, never a glyph
/// alone — a stalled screen must not be ambiguous between "thinking" and
/// "broken".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningQaStatus {
    /// Enqueued, no thread started yet.
    Pending,
    /// A headless run is in flight.
    Running,
    /// An answer arrived.
    Answered,
    /// The run failed; `LearningQa::error` carries what to do about it.
    Failed,
}

#[allow(dead_code)]
impl LearningQaStatus {
    /// Stable string stored in `learning_qa.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningQaStatus::Pending => "pending",
            LearningQaStatus::Running => "running",
            LearningQaStatus::Answered => "answered",
            LearningQaStatus::Failed => "failed",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "running" => LearningQaStatus::Running,
            "answered" => LearningQaStatus::Answered,
            "failed" => LearningQaStatus::Failed,
            _ => LearningQaStatus::Pending,
        }
    }

    /// The word shown on the row.
    pub fn word(self) -> &'static str {
        match self {
            LearningQaStatus::Pending => "queued",
            LearningQaStatus::Running => "thinking…",
            LearningQaStatus::Answered => "answered",
            LearningQaStatus::Failed => "failed",
        }
    }

    /// True while the user is still waiting on this row (drives the header's
    /// in-flight counter).
    pub fn is_in_flight(self) -> bool {
        matches!(self, LearningQaStatus::Pending | LearningQaStatus::Running)
    }
}

/// One question and its answer, anchored to a place in the project. The
/// in-memory list is the overlay's source of truth; the DB persists it when
/// one is available (mirroring the TODOs overlay).
#[derive(Debug, Clone, PartialEq)]
pub struct LearningQa {
    pub id: String,
    /// `learning_sessions.id` this row belongs to.
    pub session_id: String,
    /// Set on a follow-up: the row whose question and answer are carried into
    /// this one's prompt. Follow-ups render indented under their parent.
    ///
    /// Also set on a deep dive, which hangs under the answer it re-derives —
    /// see [`deep_dive_of`](Self::deep_dive_of) for why that one is *not* a
    /// conversational parent.
    pub parent_qa_id: Option<String>,
    /// Set only on a deep dive: the row this one re-ran.
    ///
    /// A deep dive is threaded under its origin so the two read as a pair, but
    /// it *replaces* that answer rather than continuing from it. Without this
    /// field the two relationships are indistinguishable — a follow-up on a
    /// deep dive would walk `parent_qa_id` straight back into the shallow
    /// answer the deep dive was run to check, feeding possibly-fabricated
    /// claims into the prompt that was meant to be free of them. It cannot be
    /// inferred from `run_mode` either: every Codex row is a `DeepDive` (see
    /// [`LearningRunMode::effective_for`]), including ordinary follow-ups
    /// whose ancestry must be kept.
    pub deep_dive_of: Option<String>,
    /// Repo-relative path, `None` for the project-level anchor.
    pub file_path: Option<String>,
    pub anchor: LearningAnchor,
    /// The text the anchor covered when the question was asked. Kept verbatim
    /// so the answer stays readable even after the file moves on.
    pub selection_text: String,
    /// Whether [`selection_text`](Self::selection_text) is a unified-diff
    /// excerpt. Stored rather than re-derived: a line anchor from the repo tree
    /// and one from a diff are indistinguishable once the browse scope is gone,
    /// and a follow-up needs to label its parent's capture correctly however
    /// far the file list has moved on since.
    pub selection_is_diff: bool,
    pub question: String,
    pub intent: LearningQaIntent,
    /// The level this row was answered at, so a reloaded answer explains why
    /// it reads the way it does.
    pub level: LearningLevel,
    pub answer: Option<String>,
    pub harness: AgentKind,
    pub run_mode: LearningRunMode,
    pub status: LearningQaStatus,
    /// Failure text for a `Failed` row, phrased as what to do next.
    pub error: Option<String>,
    /// `todos.id`, set only once the user explicitly made this answer
    /// actionable. Renders as `→ TODO` and makes re-invocation jump to the
    /// item instead of duplicating it.
    pub todo_id: Option<String>,
    /// `FeatureSession.id` of a live session escalated from this row.
    pub spawned_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl LearningQa {
    /// The row this one stands in for, when it is a deep dive threaded under
    /// the answer it re-derived.
    ///
    /// Ancestor traversal uses this to step *over* that row: the deep dive
    /// occupies its position in the conversation, so the answer it was run to
    /// check is not a turn that ever happened. Only honoured when it is also
    /// this row's thread parent, which is the only shape
    /// [`App::learning_deep_dive`](crate::app::App::learning_deep_dive)
    /// writes — a mismatch means the ancestry never runs through it anyway.
    pub fn superseded_id(&self) -> Option<&str> {
        match (self.deep_dive_of.as_deref(), self.parent_qa_id.as_deref()) {
            (Some(origin), Some(parent)) if origin == parent => Some(origin),
            _ => None,
        }
    }
}

/// A Learning Mode session: one per project, carrying the settings that
/// outlive a single question.
#[derive(Debug, Clone, PartialEq)]
pub struct LearningSession {
    pub id: String,
    pub project_id: String,
    /// Feature the session was opened under (its workdir is what gets read).
    pub feature_id: String,
    pub title: String,
    pub harness: AgentKind,
    pub level: LearningLevel,
    /// False until the first-open help overlay has been shown once.
    pub onboarding_seen: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Which group a file-list row belongs to.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningListGroup {
    /// The pinned orientation group shown at the top of repo-tree scope until
    /// the project has some Q&A history.
    StartHere,
    /// The ordinary file list.
    Files,
}

/// One row in Learning Mode's file list.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearningListEntry {
    /// Collapsible header for the `Start here` group.
    StartHereHeader,
    /// The repo-level orientation question — anchors to the project rather
    /// than to any file.
    ProjectTour,
    /// A directory in the repo tree. Navigation only: a directory is not a
    /// question anchor, so resting on one leaves the loaded file and the
    /// anchor exactly where they were. Only ever built in repo-tree scope.
    Dir {
        /// Repo-relative path with no trailing slash, e.g. `src/app`. This is
        /// the key `LearningViewState::expanded_dirs` stores.
        path: String,
        /// Nesting depth; 0 for a top-level directory.
        depth: usize,
        expanded: bool,
        /// Files anywhere beneath this directory, so a collapsed row can still
        /// say how much it is hiding.
        file_count: usize,
        /// Children not listed because this one directory exceeded
        /// `MAX_DIR_CHILDREN`. Non-zero rows say so rather than looking
        /// complete — the whole-listing cap this replaced had the same duty.
        truncated: usize,
    },
    /// A file. `diff_index` indexes `LearningViewState::diff_files` in
    /// branch-changes scope and is `None` in repo-tree scope. `depth` is the
    /// tree indent; it is 0 for the flat branch-changes list and for the
    /// `Start here` group, neither of which is a tree.
    File {
        path: String,
        group: LearningListGroup,
        diff_index: Option<usize>,
        depth: usize,
    },
}

#[allow(dead_code)]
impl LearningListEntry {
    /// The repo-relative path this row loads, if it loads one. Deliberately
    /// `None` for a directory: this is what the content pane and the anchor
    /// follow, and a directory must move neither.
    pub fn path(&self) -> Option<&str> {
        match self {
            LearningListEntry::File { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// The directory this row is, if it is one.
    pub fn dir_path(&self) -> Option<&str> {
        match self {
            LearningListEntry::Dir { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// A stable identity for the row, used to put the cursor back on the same
    /// thing after the list is rebuilt. Unlike `path()` this covers
    /// directories, because collapsing one must leave the cursor on it.
    pub fn row_key(&self) -> Option<(bool, &str)> {
        match self {
            LearningListEntry::Dir { path, .. } => Some((true, path.as_str())),
            LearningListEntry::File { path, .. } => Some((false, path.as_str())),
            _ => None,
        }
    }

    /// How far the row is indented in the tree.
    pub fn depth(&self) -> usize {
        match self {
            LearningListEntry::Dir { depth, .. } | LearningListEntry::File { depth, .. } => *depth,
            _ => 0,
        }
    }

    /// Whether the cursor can rest here (group headers are skipped by
    /// navigation only when collapsed — they stay selectable so the group can
    /// be expanded again).
    pub fn is_file(&self) -> bool {
        matches!(self, LearningListEntry::File { .. })
    }
}

/// Which pane has focus in the Learning Mode overlay.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningFocus {
    FileList,
    Content,
    Qa,
}

/// An open starter-question picker: indices into
/// `crate::app::learning::STARTER_QUESTIONS`, filtered to the ones that make
/// sense for the current anchor. Picking one fills the prompt so it can still
/// be edited before it's asked.
#[allow(dead_code)]
pub struct LearningStarterPicker {
    pub indices: Vec<usize>,
    pub selected: usize,
}

/// Which harness answers questions from here on. Pre-selected when the
/// overlay opens, so this only exists while the user is actively changing it.
#[allow(dead_code)]
pub struct LearningHarnessPicker {
    pub harnesses: Vec<AgentKind>,
    pub selected: usize,
}

/// An open question prompt. The same editor serves both intents and
/// follow-ups; the title bar shows the resolved anchor and chosen intent.
#[allow(dead_code)]
pub struct LearningQuestionEditor {
    pub editor: TextEditor,
    pub intent: LearningQaIntent,
    /// Set when this prompt is a follow-up to an existing row.
    pub parent_qa_id: Option<String>,
    /// Anchor captured when the prompt opened, so browsing can't move it.
    pub anchor: LearningAnchor,
    pub file_path: Option<String>,
    /// The anchored text captured alongside `anchor`.
    pub selection_text: String,
    /// Whether `selection_text` is a unified-diff excerpt, captured with it so
    /// the prompt labels it the same way however the browse scope changes
    /// before the question is submitted.
    pub selection_is_diff: bool,
    pub scroll: usize,
    pub sync_to_cursor: bool,
}

/// An open "add this answer to the project's TODO list" confirmation.
///
/// Lives inside [`LearningViewState`] like the pickers rather than as its own
/// `AppMode`, so cancelling returns to exactly the browsing state underneath.
/// Nothing is written until it is confirmed: the seeded title is a guess, and
/// this mode's audience is the least likely to notice a wrong one going in
/// behind their back.
#[allow(dead_code)]
pub struct LearningActionEditor {
    /// The Q&A row the item is being made from.
    pub qa_id: String,
    /// Editable title, seeded from the answer. An explanation has no one-line
    /// summary in it, so the seed there is a truncation the user is expected to
    /// fix — which is most of why this dialog exists at all.
    pub title: TextEditor,
    /// The note's body: where the question was anchored, what was asked, and an
    /// excerpt of the answer. Shown but not edited here, so what gets written
    /// is never a surprise.
    pub body: String,
    /// Refusal raised by a key pressed *in* the dialog (an emptied title). Kept
    /// here rather than on the overlay because the dialog covers the overlay's
    /// banner line.
    pub error: Option<String>,
    pub scroll: usize,
    pub sync_to_cursor: bool,
}

/// State for the Learning Mode overlay (`AppMode::Learning`) — a read-only
/// file browser over a project, with an agent answering questions about
/// whatever the cursor is on. Nothing in this mode writes to the repository.
#[allow(dead_code)]
pub struct LearningViewState {
    /// Project being studied.
    pub project_id: String,
    /// Project / feature indices the overlay was opened from, used to resolve
    /// the feature for escalation and to restore dashboard selection on close.
    pub pi: usize,
    pub fi: usize,
    /// Display labels for the header.
    pub project_name: String,
    pub feature_name: String,
    /// The feature's working directory — everything is read from here.
    pub workdir: PathBuf,
    /// False for non-git projects, where branch-changes scope has no meaning
    /// and the file list falls back to a capped plain walk.
    pub is_git: bool,
    pub scope: BrowseScope,
    /// File-list rows in display order (`Start here` group first, when shown).
    pub entries: Vec<LearningListEntry>,
    pub selected_entry: usize,
    pub list_scroll: usize,
    pub start_here_collapsed: bool,
    /// Which repo-tree directories are expanded, by repo-relative path. The
    /// tree is rebuilt from this on every reload, so it — not `entries` — is
    /// what expansion state actually lives in. Seeded on open with the
    /// ancestors of the `Start here` candidates, so `src/` is open at the file
    /// a newcomer is most likely to want.
    pub expanded_dirs: std::collections::BTreeSet<String>,
    /// Whether that seeding has happened. It runs once per overlay, so a later
    /// reload can't re-open a directory the user deliberately closed.
    pub expanded_seeded: bool,
    /// The repo's flat path list, kept so expanding or collapsing a directory
    /// rebuilds `entries` from memory instead of shelling out to `git ls-files`
    /// again. `entries` is derived from this plus `expanded_dirs`; this is the
    /// input, and it only changes when the listing is genuinely re-read.
    pub repo_files: Vec<String>,
    /// The surviving `Start here` candidates, cached for the same reason.
    pub start_here: Vec<String>,
    /// Diff snapshot backing `BrowseScope::BranchChanges`.
    pub diff_files: Vec<crate::diff::DiffFile>,
    /// Lines of the loaded file, and the path they came from.
    pub content: Vec<String>,
    pub content_path: Option<String>,
    pub content_scroll: usize,
    /// Why the selected file could not be shown (binary, too large, unreadable).
    pub content_error: Option<String>,
    /// Cursor into the content pane: a 0-based index into `content` in
    /// repo-tree scope, or into the file's `addressable_lines()` in
    /// branch-changes scope.
    pub cursor_line: usize,
    /// Start of an in-progress multi-line selection; `None` selects only the
    /// cursor line.
    pub selection_anchor: Option<usize>,
    /// The anchor a question would currently be asked against.
    pub anchor: LearningAnchor,
    pub focus: LearningFocus,
    /// Open question prompt, if any.
    pub question: Option<LearningQuestionEditor>,
    /// Q&A history for this project, oldest first, follow-ups after parents.
    pub qa: Vec<LearningQa>,
    /// Anchors that no longer point where they were stored, by `LearningQa::id`.
    ///
    /// Deliberately a side table rather than a field on the row: a verdict is a
    /// judgment about the working directory as it is right now, not something
    /// the row carries, and keeping the two apart is what stops it being
    /// written back over the range the question was actually asked at. A row
    /// with no entry here is anchored as stored.
    pub anchor_drift: std::collections::HashMap<String, LearningAnchorDrift>,
    pub selected_qa: usize,
    pub qa_scroll: usize,
    /// Answer pane state — offset plus the render cache
    /// `draw_markdown_document` needs.
    pub answer_open: bool,
    pub answer_scroll: usize,
    pub answer_rendered_width: u16,
    pub answer_rendered_lines: Vec<ratatui::text::Line<'static>>,
    /// Harness answering questions. Pre-selected, so the picker is optional.
    pub harness: AgentKind,
    /// Open harness picker, if any. Lives inside the overlay rather than as
    /// its own `AppMode` so opening it can't lose the browsing state behind it.
    pub harness_picker: Option<LearningHarnessPicker>,
    /// Open starter-question picker, if any.
    pub starter_picker: Option<LearningStarterPicker>,
    /// Open "add this to the TODO list" confirmation, if any.
    pub action_editor: Option<LearningActionEditor>,
    pub level: LearningLevel,
    /// `learning_sessions.id` backing this overlay.
    pub session_id: String,
    /// True while the `?` help overlay is open (also shown automatically on
    /// first open, per `onboarding_seen`).
    pub help_open: bool,
    pub help_scroll: usize,
    /// Transient error banner (file load, DB, run dispatch).
    pub error: Option<String>,
    /// Transient confirmation banner — what a key just *did*, as opposed to
    /// why it refused. Shares the error's line but not its colour: telling
    /// someone their entry was re-filed in the failure red is its own small
    /// lie, and this mode's audience is the least equipped to discount it.
    pub notice: Option<String>,
    /// The Q&A row `notice` was raised on, when it describes one. The wording
    /// is only true of that row as it stood at the keypress ("the answer on
    /// its way was asked for as an explanation"), so the banner is dropped
    /// when the cursor leaves the row or the row's run lands.
    pub notice_qa_id: Option<String>,
}

#[allow(dead_code)]
impl LearningViewState {
    /// How many answers are still generating — shown in the header so a slow
    /// run reads as progress rather than a hang.
    pub fn in_flight_count(&self) -> usize {
        self.qa.iter().filter(|q| q.status.is_in_flight()).count()
    }

    /// The path a question would anchor to, `None` for the project anchor.
    pub fn anchor_path(&self) -> Option<&str> {
        match self.anchor {
            LearningAnchor::Project => None,
            _ => self.content_path.as_deref(),
        }
    }

    /// The currently selected file-list entry.
    pub fn selected_entry(&self) -> Option<&LearningListEntry> {
        self.entries.get(self.selected_entry)
    }

    /// The `DiffFile` behind the selected entry, in branch-changes scope.
    pub fn selected_diff_file(&self) -> Option<&crate::diff::DiffFile> {
        match self.entries.get(self.selected_entry) {
            Some(LearningListEntry::File {
                diff_index: Some(i),
                ..
            }) => self.diff_files.get(*i),
            _ => None,
        }
    }

    /// Whether hunk selection is available — it needs a diff, so repo-tree
    /// scope has none.
    pub fn hunk_selection_available(&self) -> bool {
        self.scope == BrowseScope::BranchChanges && self.selected_diff_file().is_some()
    }

    /// The Q&A row under the history cursor.
    pub fn selected_qa(&self) -> Option<&LearningQa> {
        self.qa.get(self.selected_qa)
    }

    /// Drop the confirmation banner and whatever row it was raised on.
    pub fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_qa_id = None;
    }

    /// Move the history cursor to `index`, dropping a banner raised on the row
    /// being left.
    ///
    /// A notice describes the row it was raised on ("re-filed as a change
    /// request"), so it must not follow the cursor onto a different entry and
    /// appear to describe that one instead. Every cursor move goes through
    /// here — including the programmatic ones (a follow-up selecting its new
    /// row, a deep dive jumping to the one that already exists), which is
    /// where a notice would otherwise survive untouched.
    pub fn select_qa(&mut self, index: usize) {
        if index != self.selected_qa {
            self.clear_notice();
        }
        self.selected_qa = index;
    }
}
