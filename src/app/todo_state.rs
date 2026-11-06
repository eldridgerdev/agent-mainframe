use super::*;
use crate::editor::TextEditor;

/// What an active TODOs inline edit targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoEditTarget {
    /// Adding a brand-new item (its title).
    New,
    /// Editing the selected item's title.
    Title,
    /// Editing the selected item's notes/detail body (multi-line).
    Notes,
    /// Editing the list's free-form scratchpad note (persisted in the
    /// legacy `carry_over` column).
    Scratchpad,
}

/// An in-progress inline edit within the TODOs overlay.
pub struct TodoEditor {
    pub target: TodoEditTarget,
    pub editor: TextEditor,
}

/// Which scope a pane of the TODOs overlay shows. The variants are in the
/// order the panes are laid out and the order ties between them resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoPaneKind {
    /// This feature's own checkout. Absent for a feature sitting on the repo
    /// root, which has no worktree of its own to scope a list to.
    Worktree,
    /// The project's list — the only scope that existed before scoping.
    Project,
    /// The machine-wide list, belonging to no project.
    Global,
}

impl TodoPaneKind {
    /// The pane header, and the noun used when a message has to name a scope.
    pub fn label(self) -> &'static str {
        match self {
            TodoPaneKind::Worktree => "Worktree",
            TodoPaneKind::Project => "Project",
            TodoPaneKind::Global => "Global",
        }
    }
}

/// One scope's list within the TODOs overlay: its own items, cursor, scroll,
/// and scratchpad, so switching focus never disturbs the pane being left.
pub struct TodoPane {
    pub kind: TodoPaneKind,
    /// The scope this pane reads and writes. Carries the project id and
    /// workdir, so it is the whole key to the pane's list.
    pub scope: crate::db::todos::TodoScope,
    /// Name shown in the pane header beside the scope label (the worktree's
    /// feature name, the project name, or nothing for global).
    pub title: String,
    /// The loaded list (scratchpad note, id). `None` until the list exists —
    /// with no DB (tests) it is synthesized on first write, and with one it is
    /// created lazily, so an untouched scope leaves no row behind.
    pub list: Option<crate::db::todos::TodoList>,
    /// Items in display order (open first, then by sort_order).
    pub todos: Vec<crate::db::todos::Todo>,
    /// Cursor into `todos`.
    pub selected: usize,
    /// Vertical scroll offset into this pane's list area.
    pub scroll_offset: usize,
}

impl TodoPane {
    pub fn selected_todo(&self) -> Option<&crate::db::todos::Todo> {
        self.todos.get(self.selected)
    }

    /// The list's free-form scratchpad note (stored in the legacy
    /// `carry_over` column), when it is not blank.
    pub fn scratchpad(&self) -> Option<&str> {
        self.list
            .as_ref()
            .and_then(|l| l.carry_over.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// State for the native TODOs overlay (`AppMode::Todos`): up to three scoped
/// panes, which one has focus, and any in-progress edit or prompt layered over
/// them.
pub struct TodoViewState {
    /// Project / feature indices the TODOs session lives under, used to
    /// resolve the session for selection on close and to host new lists.
    pub pi: usize,
    pub fi: usize,
    /// Display labels for the header.
    pub project_name: String,
    pub feature_name: String,
    /// The scoped panes, always ordered worktree → project → global. The
    /// worktree pane is absent for a feature on the repo root, which is the
    /// only way this is shorter than three.
    pub panes: Vec<TodoPane>,
    /// Index into `panes` of the visible pane that owns the cursor. `None` is
    /// valid for a repository-root feature when both optional scopes are
    /// hidden.
    pub focus: Option<usize>,
    /// Active inline edit, if any (add/edit title/notes/scratchpad).
    pub editor: Option<TodoEditor>,
    /// Whether inline edits open with the vim keymap. Remembered on the state
    /// (not the editor, which is rebuilt for each edit) for the life of the
    /// overlay; a fresh overlay starts with vim off, matching the other opt-in
    /// editor surfaces (compose, final review). Toggled with `Ctrl+T`.
    pub todo_vim_enabled: bool,
    /// Set when a delete is awaiting y/n confirmation.
    pub pending_delete: bool,
    /// Active launch step (chooser / destination), layered over the list.
    pub launch: Option<TodoLaunchStep>,
    /// Active move/copy scope chooser (`M` / `C`), layered the same way.
    pub scope_move: Option<TodoScopeMoveState>,
}

impl TodoViewState {
    pub fn pane_is_visible(pane: &TodoPane, project_visible: bool, global_visible: bool) -> bool {
        match pane.kind {
            TodoPaneKind::Worktree => true,
            TodoPaneKind::Project => project_visible,
            TodoPaneKind::Global => global_visible,
        }
    }

    /// Indices of actionable panes in worktree → project → global order.
    pub fn visible_pane_indices(&self, project_visible: bool, global_visible: bool) -> Vec<usize> {
        self.panes
            .iter()
            .enumerate()
            .filter_map(|(index, pane)| {
                Self::pane_is_visible(pane, project_visible, global_visible).then_some(index)
            })
            .collect()
    }

    pub fn focused(&self) -> Option<&TodoPane> {
        self.focus.and_then(|focus| self.panes.get(focus))
    }

    pub fn focused_mut(&mut self) -> Option<&mut TodoPane> {
        self.focus.and_then(|focus| self.panes.get_mut(focus))
    }
}

/// The scope chooser raised by `M` (move) and `C` (copy) over the selected
/// TODO. Targets are pane indices rather than scopes so the in-memory pane and
/// the persisted list are updated from one lookup.
pub struct TodoScopeMoveState {
    /// `true` for a copy (leaves the original in place, unstarted), `false`
    /// for a move (re-files the same item, links and all).
    pub copy: bool,
    /// The item being re-filed, by id, so the list changing underneath the
    /// prompt is noticed rather than acted on stale.
    pub todo_id: String,
    pub todo_title: String,
    /// Candidate destinations as `(label, pane index)` — every pane but the
    /// one the item is already in.
    pub targets: Vec<(String, usize)>,
    pub selected: usize,
}

impl TodoScopeMoveState {
    pub fn move_cursor(&mut self, delta: isize) {
        if self.targets.is_empty() {
            return;
        }
        let last = self.targets.len() as isize - 1;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }
}

/// Single-line quick-capture of a TODO from inside a session view. The typed
/// title is appended to the session feature's own worktree list — falling back
/// to the project list when that feature sits on the repo root — auto-creating
/// the list (and a TODOs session under the current feature) when there is none
/// yet. `view` is the session view to return to on commit/cancel.
pub struct TodoQuickCaptureState {
    pub view: ViewState,
    /// Name of the project the TODO will be added to (shown in the dialog).
    pub project_name: String,
    /// Which list this capture will land in, named in the overlay so the
    /// target is never a guess (e.g. `"Worktree · add-login"`).
    pub list_label: String,
    /// The title being typed.
    pub input: String,
}

/// Collects the user's instruction before starting a fresh-context agent
/// session (`Ctrl+Space` then `Shift+F`, `crate::app::handoff`), so the
/// seeded prompt is complete on arrival instead of asking the user to type
/// over a placeholder in the new session's compose box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshContextPromptSource {
    Manual,
    ContextHint,
}

pub struct FreshContextPromptState {
    pub view: ViewState,
    /// Feature the fresh session will be created in, shown in the dialog.
    pub feature_name: String,
    /// The instruction being typed.
    pub input: String,
    /// Whether the input came from the context-hint continuation generator.
    pub source: FreshContextPromptSource,
}

/// Prompt shown when the feature that hosts a project's TODO list is deleted
/// while the project survives (see `docs/backlog/feature-todos-plan.md`, Epic 1).
/// The user chooses which surviving feature re-homes the list, or deletes it.
pub struct TodosHostReassignState {
    /// Project whose list is being re-homed.
    pub project_name: String,
    /// Name of the just-deleted host feature (shown in the prompt).
    pub deleted_feature_name: String,
    /// `todo_lists.id` of the orphaned list.
    pub list_id: String,
    /// Surviving features the list can be re-homed onto: `(name, feature_id)`.
    pub candidates: Vec<(String, String)>,
    /// Selected option index: `0..candidates.len()` re-homes onto that feature;
    /// `== candidates.len()` deletes the list.
    pub selected: usize,
    /// Number of TODOs in the list (shown so the user knows what's at stake).
    pub todo_count: usize,
}

/// Which TODO a plan-mode run was started from, carried through the interview
/// so accepting the plan can link the result back to the row it came from.
///
/// The list id rides along with the todo id because the overlay may be closed
/// by the time the plan is accepted — a new-feature run leaves the Todos mode
/// entirely — so the link has to be written straight to the DB rather than
/// through the in-memory list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoPlanOrigin {
    pub todo_id: String,
    pub list_id: String,
    /// The TODO's title when the run started. Used for the interview header
    /// and to name the plan file, so it is captured rather than re-read: the
    /// title can be edited while the interview runs.
    pub todo_title: String,
    /// The feature hosting the TODO list when the run started. The host-feature
    /// destination spawns its session here; the new-feature destination
    /// ignores it.
    pub host_feature_id: String,
}

/// What `g`/`Enter` on an unlinked TODO offers: spawn in an existing feature,
/// spawn in a fresh worktree, or run a plan interview first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoLaunchAction {
    /// Today's behavior: an agent session in the host feature (or, for a
    /// project/global TODO, a feature the user picks), composer seeded with the
    /// TODO, editable and unsent.
    SpawnSession,
    /// Create a new AMF feature and git worktree for this TODO — no plan
    /// interview — then seed its agent with the TODO.
    SpawnInNewFeature,
    /// Run the guided plan interview with the TODO as its brief.
    PlanMode,
}

impl TodoLaunchAction {
    pub const ALL: [TodoLaunchAction; 3] = [
        TodoLaunchAction::SpawnSession,
        TodoLaunchAction::SpawnInNewFeature,
        TodoLaunchAction::PlanMode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodoLaunchAction::SpawnSession => "Start an agent on this TODO",
            TodoLaunchAction::SpawnInNewFeature => "Start an agent in a new feature",
            TodoLaunchAction::PlanMode => "Plan this TODO first",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            TodoLaunchAction::SpawnSession => {
                "Opens a session in this feature with the TODO in the composer, unsent."
            }
            TodoLaunchAction::SpawnInNewFeature => {
                "Creates a new branch and worktree, then seeds its agent with the TODO, unsent."
            }
            TodoLaunchAction::PlanMode => {
                "Runs the discovery interview, then starts work from the plan you accept."
            }
        }
    }
}

/// Where an accepted TODO plan lands. Chosen up front, before the interview, so
/// the plan is written into the worktree it is actually for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoPlanDestination {
    /// The feature hosting the TODO list. Accepting spawns a seeded session
    /// there; nothing new is checked out.
    HostFeature,
    /// A new AMF feature and git worktree, created through the ordinary
    /// create-feature wizard with plan mode on.
    NewFeature,
}

impl TodoPlanDestination {
    pub const ALL: [TodoPlanDestination; 2] = [
        TodoPlanDestination::HostFeature,
        TodoPlanDestination::NewFeature,
    ];
}

/// The four answers to "this TODO already has work started for it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoImplementChoice {
    /// Go to the feature or session the earlier run created.
    Jump,
    /// Start a second agent on it anyway, in the host feature.
    SpawnNew,
    /// Leave this one alone and scan on for the next candidate.
    SkipToNext,
    /// Change nothing and return to where the key was pressed.
    Cancel,
}

impl TodoImplementChoice {
    pub const ALL: [TodoImplementChoice; 4] = [
        TodoImplementChoice::Jump,
        TodoImplementChoice::SpawnNew,
        TodoImplementChoice::SkipToNext,
        TodoImplementChoice::Cancel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodoImplementChoice::Jump => "Go to the work already started",
            TodoImplementChoice::SpawnNew => "Start another agent on it",
            TodoImplementChoice::SkipToNext => "Skip it and take the next TODO",
            TodoImplementChoice::Cancel => "Cancel",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            TodoImplementChoice::Jump => {
                "Opens the feature or session this TODO was launched into."
            }
            TodoImplementChoice::SpawnNew => {
                "Opens a second session in the host feature, composer seeded and unsent."
            }
            TodoImplementChoice::SkipToNext => {
                "Leaves this one as it is and scans on for the next candidate."
            }
            TodoImplementChoice::Cancel => "Changes nothing.",
        }
    }
}

/// "Implement next" found that its best candidate already has work started for
/// it (`AppMode::TodoImplementChoice`).
///
/// Reached from two surfaces — a TODOs session row on the dashboard and the
/// TODOs overlay — so it is an [`AppMode`] of its own rather than a step
/// layered inside [`TodoViewState`], which only one of the two has. `origin`
/// is the mode the key was pressed in, restored verbatim on every exit: from
/// the overlay that returns the list with its cursor, scroll, and any unsaved
/// in-memory rows intact.
pub struct TodoImplementChoiceState {
    /// The mode to return to. Boxed because it may be a `Todos` overlay, whose
    /// state is large and (holding a `TextEditor`) not clonable.
    pub origin: Box<AppMode>,
    /// Project and fallback feature indices the scan ran under.
    pub pi: usize,
    pub fallback_fi: usize,
    /// The list's host feature id, when the list could be loaded.
    pub host_feature_id: Option<String>,
    /// Which scope the candidate came from. Decides whether *Start another
    /// agent on it* can spawn straight away (worktree) or has to ask which
    /// feature to spawn in (project / global).
    pub pane_kind: TodoPaneKind,
    /// The candidate: its id, so it is re-resolved by id alone on confirm
    /// (not trusted, and not tied to a remembered list — the item may have
    /// been moved to a different list while this prompt was open), and its
    /// title for display.
    pub todo_id: String,
    pub todo_title: String,
    /// TODOs already passed over by *Skip to next*, carried so a resumed scan
    /// does not offer the same item again. Ids, not indices: the list may be
    /// reordered underneath.
    pub skipped_ids: Vec<String>,
    pub selected: usize,
}

impl TodoImplementChoiceState {
    pub fn move_cursor(&mut self, delta: isize) {
        let last = TodoImplementChoice::ALL.len().saturating_sub(1) as isize;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }

    pub fn choice(&self) -> TodoImplementChoice {
        TodoImplementChoice::ALL[self.selected.min(TodoImplementChoice::ALL.len() - 1)]
    }
}

/// A feature to put an agent on a project- or global-scoped TODO in
/// (`AppMode::TodoSpawnTarget`).
///
/// A whole [`AppMode`] rather than a step inside [`TodoViewState`] for the
/// same reason [`TodoImplementChoiceState`] is one: the dashboard's "implement
/// next" reaches it with no overlay open. `origin` is the mode the key was
/// pressed in, restored verbatim on cancel, so the prompt never costs the user
/// their place in the list.
pub struct TodoSpawnTargetState {
    pub origin: Box<AppMode>,
    /// The item to spawn on, carried by value: the overlay it came from may
    /// not be open, and a global TODO's list is not reachable from `pi`.
    pub todo: crate::db::todos::Todo,
    /// Which scope the TODO came from, shown so the user knows why they are
    /// being asked.
    pub pane_kind: TodoPaneKind,
    /// Candidates as `(label, pi, fi)`. A project-scoped TODO lists that
    /// project's features; a global one lists every project's.
    pub candidates: Vec<(String, usize, usize)>,
    pub selected: usize,
    /// Set when the user explicitly asked for a second agent on an item that
    /// already has one, so the spawn does not reuse the existing session.
    pub force_new: bool,
}

impl TodoSpawnTargetState {
    pub fn move_cursor(&mut self, delta: isize) {
        if self.candidates.is_empty() {
            return;
        }
        let last = self.candidates.len() as isize - 1;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.candidates
            .get(self.selected)
            .map(|(_, pi, fi)| (*pi, *fi))
    }
}

/// What to do with the unfinished TODOs in a worktree list whose feature is
/// being deleted (`AppMode::TodoDeleteDisposition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoDeleteDisposition {
    MoveToProject,
    MoveToGlobal,
    Delete,
    Cancel,
}

impl TodoDeleteDisposition {
    pub const ALL: [TodoDeleteDisposition; 4] = [
        TodoDeleteDisposition::MoveToProject,
        TodoDeleteDisposition::MoveToGlobal,
        TodoDeleteDisposition::Delete,
        TodoDeleteDisposition::Cancel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodoDeleteDisposition::MoveToProject => "Move them to the project list",
            TodoDeleteDisposition::MoveToGlobal => "Move them to the global list",
            TodoDeleteDisposition::Delete => "Delete them with the worktree",
            TodoDeleteDisposition::Cancel => "Cancel — keep the feature",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            TodoDeleteDisposition::MoveToProject => {
                "They stay with the project and show up in its pane."
            }
            TodoDeleteDisposition::MoveToGlobal => {
                "They leave the project for the machine-wide list."
            }
            TodoDeleteDisposition::Delete => "The items go away for good, with the list.",
            TodoDeleteDisposition::Cancel => {
                "Nothing is deleted — the feature and its worktree stay."
            }
        }
    }
}

/// The prompt raised before a feature is deleted while its worktree list still
/// has unfinished items. Deleting a worktree is hard to reverse, so this is a
/// blocking prompt: nothing is killed or removed until a choice is made, and
/// *Cancel* returns to the dashboard with the feature intact.
pub struct TodoDeleteDispositionState {
    pub project_name: String,
    pub feature_name: String,
    /// The doomed feature's id. Carried so *Move to the project list* can host
    /// a newly-created project list on a feature that will still be there
    /// afterwards — hosting it on this one would hand the items straight to
    /// the orphaned-list cleanup that runs once the deletion completes.
    pub feature_id: String,
    /// The worktree list and where it lives, so the disposition can be applied
    /// without re-deriving it from indices that the deletion will invalidate.
    pub project_id: String,
    pub workdir: String,
    pub list_id: String,
    /// How many items are still open, stated in the prompt.
    pub unfinished: usize,
    pub selected: usize,
}

impl TodoDeleteDispositionState {
    pub fn move_cursor(&mut self, delta: isize) {
        let last = TodoDeleteDisposition::ALL.len() as isize - 1;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }

    pub fn choice(&self) -> TodoDeleteDisposition {
        TodoDeleteDisposition::ALL[self.selected.min(TodoDeleteDisposition::ALL.len() - 1)]
    }
}

/// A modal step layered over the TODO list, the way `pending_delete` and
/// `editor` are: the list, cursor, and scroll stay intact underneath, so `Esc`
/// returns to exactly what the user was looking at.
///
/// A separate [`AppMode`] would replace [`TodoViewState`] wholesale and force
/// the list to be reloaded — and the in-memory list is the overlay's source of
/// truth, so it would also lose unsaved state on a DB-less run.
#[derive(Debug, Clone)]
pub enum TodoLaunchStep {
    /// `g`/`Enter` on a TODO with no existing link: spawn, or plan first.
    Choice {
        origin: TodoPlanOrigin,
        selected: usize,
    },
    /// Where an accepted plan should land. Asked before the interview so the
    /// plan is written into the worktree it is actually for.
    Destination {
        origin: TodoPlanOrigin,
        /// Name of the feature hosting the list, shown on the first option.
        host_feature_name: String,
        /// False when the project has no git repository, which makes a new
        /// worktree impossible. The option is still shown, and says why.
        can_create_worktree: bool,
        selected: usize,
    },
}

impl TodoLaunchStep {
    pub fn origin(&self) -> &TodoPlanOrigin {
        match self {
            TodoLaunchStep::Choice { origin, .. } => origin,
            TodoLaunchStep::Destination { origin, .. } => origin,
        }
    }

    pub fn selected(&self) -> usize {
        match self {
            TodoLaunchStep::Choice { selected, .. } => *selected,
            TodoLaunchStep::Destination { selected, .. } => *selected,
        }
    }

    pub fn option_count(&self) -> usize {
        match self {
            TodoLaunchStep::Choice { .. } => TodoLaunchAction::ALL.len(),
            TodoLaunchStep::Destination { .. } => TodoPlanDestination::ALL.len(),
        }
    }

    /// Move the cursor, clamped rather than wrapped: with two options, wrapping
    /// makes `j` and `k` the same key and the highlight appears not to move.
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.option_count().saturating_sub(1);
        let current = self.selected() as isize;
        let next = (current + delta).clamp(0, last as isize) as usize;
        match self {
            TodoLaunchStep::Choice { selected, .. } => *selected = next,
            TodoLaunchStep::Destination { selected, .. } => *selected = next,
        }
    }

    /// The chosen action, or `None` on the destination step.
    pub fn action(&self) -> Option<TodoLaunchAction> {
        match self {
            TodoLaunchStep::Choice { selected, .. } => {
                Some(TodoLaunchAction::ALL[(*selected).min(TodoLaunchAction::ALL.len() - 1)])
            }
            TodoLaunchStep::Destination { .. } => None,
        }
    }

    /// The chosen destination, or `None` on the chooser step.
    pub fn destination(&self) -> Option<TodoPlanDestination> {
        match self {
            TodoLaunchStep::Destination { selected, .. } => {
                Some(TodoPlanDestination::ALL[(*selected).min(TodoPlanDestination::ALL.len() - 1)])
            }
            TodoLaunchStep::Choice { .. } => None,
        }
    }
}

/// The view to return to plus the stable TODO identity to complete.
pub struct TodoReferenceCompletionState {
    pub view: ViewState,
    pub todo_id: String,
}
