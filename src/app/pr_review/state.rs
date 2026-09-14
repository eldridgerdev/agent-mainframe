use crate::editor::TextEditor;
use crate::project::{AgentKind, VibeMode};
use crate::token_tracking::{SessionTokenUsage, TokenUsageSource};
use std::collections::HashMap;
use std::path::PathBuf;

/// Transient state while a PR's comments are being fetched off the UI thread.
#[derive(Debug, Clone)]
pub struct PrReviewLoadState {
    /// Working directory of the feature whose PR we're reviewing.
    pub workdir: PathBuf,
    /// The resolved PR being loaded.
    pub pr: crate::github::PrRef,
    /// Usage snapshots carried through a manual refresh so refreshing comments
    /// does not restart the current triage-visit tally.
    pub usage_baselines: HashMap<TokenUsageSource, SessionTokenUsage>,
}

/// Manual PR-number override prompt: shown when the branch has no detectable
/// open PR (or the user wants to review a different one). Collects a number,
/// then resolves it via `gh pr view <n>` and starts the comment fetch.
#[derive(Debug, Clone)]
pub struct PrNumberPromptState {
    /// Working directory of the feature whose PR we're reviewing.
    pub workdir: PathBuf,
    /// Digits typed so far.
    pub input: String,
    /// Last resolve failure, shown inline so the user can correct and retry.
    pub error: Option<String>,
}

/// PR picker: a selectable list of the repo's pull requests, so the user can
/// open a PR for review without knowing its number. Reached when the branch has
/// no auto-detectable PR, or on demand from PR Triage to switch PRs. The
/// manual number prompt stays one keypress away (`#`).
#[derive(Debug, Clone)]
pub struct PrPickerState {
    /// Working directory of the feature whose repo we're listing PRs for.
    pub workdir: PathBuf,
    /// The fetched PR rows (newest-updated first).
    pub entries: Vec<crate::github::PrListEntry>,
    /// Index of the highlighted row.
    pub selected: usize,
    /// When true the list includes closed/merged PRs (`gh pr list --state all`);
    /// otherwise open-only. Toggled with `a`.
    pub include_closed: bool,
    /// Last fetch/resolve failure, shown inline.
    pub error: Option<String>,
    /// When `Some`, the lookback-bootstrap depth picker (`b`) is open over the
    /// picker.
    pub bootstrap_pick: Option<BootstrapPickState>,
    /// When `Some`, the review-memory compact confirm overlay (`c`) is open
    /// over the picker.
    pub compact_confirm: Option<CompactConfirmState>,
    /// The logged-in `gh` user's login, when resolvable — used to highlight
    /// the user's own PRs in the row rendering. `None` if unresolved/failed.
    pub current_user: Option<String>,
}

/// Depth picker for the review-memory lookback bootstrap (`b` in the PR
/// picker): pick how many recent merged/closed PRs to learn from before
/// running the fetch + distill pass.
#[derive(Debug, Clone)]
pub struct BootstrapPickState {
    /// Index into [`crate::app::pr_review::BootstrapDepth::ALL`].
    pub selected: usize,
    /// Which doc the distilled findings land in, toggled with `g`. Defaults to
    /// `Project`: a bootstrap learns from *this* repo's PR history, so its
    /// findings belong to this repo unless the user says otherwise.
    pub scope: crate::app::review_memory::MemoryScope,
}

/// Full-screen progress view for the lookback bootstrap's background fetch +
/// distill pass, entered once a depth is confirmed.
#[derive(Debug, Clone)]
pub struct BootstrapRunState {
    /// The PR picker to return to on completion or cancel.
    pub origin: PrPickerState,
    pub depth: crate::app::pr_review::BootstrapDepth,
    /// Which doc the run is appending to, carried through from the picker so
    /// the running screen and completion toast can name it.
    pub scope: crate::app::review_memory::MemoryScope,
    pub stage: crate::app::pr_review::BootstrapStage,
}

/// Confirm overlay for the review-memory compact pass (`c` in the PR picker):
/// shows how many findings are currently in the doc before spending an agent
/// pass to merge near-duplicates and prune stale ones (Epic E "prevent
/// review-memory rot").
#[derive(Debug, Clone)]
pub struct CompactConfirmState {
    /// Bullet count in the doc as it stands, read synchronously when the
    /// overlay opens (a local file read — cheap enough not to background).
    /// Re-read for the newly selected doc on every `scope` toggle, so the
    /// number always describes what `⏎` would actually compact.
    pub existing_findings: usize,
    /// Which doc gets compacted, toggled with `g`. Defaults to `Project` when
    /// that doc has findings, otherwise `Global` — so `c` still reaches the
    /// only non-empty doc without the user having to know to press `g`.
    pub scope: crate::app::review_memory::MemoryScope,
}

/// Full-screen progress view for the review-memory compact pass's background
/// read + rewrite, entered once the confirm overlay is accepted.
#[derive(Debug, Clone)]
pub struct CompactRunState {
    /// The PR picker to return to on completion or cancel.
    pub origin: PrPickerState,
    /// Resolved path of the review-memory doc being compacted, carried
    /// through from confirm so the poll's success path doesn't need to
    /// re-resolve it (a second `repo_root` lookup) once the background
    /// thread reports back.
    pub path: PathBuf,
    /// Which doc the run is rewriting, carried through from confirm so the
    /// running screen can name it (the path alone doesn't read as
    /// project-vs-global at a glance).
    pub scope: crate::app::review_memory::MemoryScope,
    pub stage: crate::app::pr_review::CompactStage,
}

/// Full-screen review of the compact pass's proposed replacement doc, entered
/// once the background run finishes. Unlike [`append_finding`]-backed dialogs
/// (`M`, the bootstrap), this proposes rewriting the *entire* doc, so nothing
/// is written until the user explicitly confirms here — editable first, same
/// as every other write in this pane.
///
/// [`append_finding`]: crate::app::review_memory::append_finding
#[derive(Debug, Clone)]
pub struct CompactReviewState {
    /// The PR picker to return to on write or discard.
    pub origin: PrPickerState,
    /// Resolved path of the review-memory doc this will write to.
    pub path: PathBuf,
    /// Which doc is being rewritten, so the success toast names it.
    pub scope: crate::app::review_memory::MemoryScope,
    /// Bullet count in the doc before compacting, for the "N -> M" summary.
    pub original_findings: usize,
    /// Bullet count in the agent's proposed replacement, for the same summary.
    pub proposed_findings: usize,
    /// The proposed replacement text, editable before writing.
    pub editor: TextEditor,
    /// The doc exactly as the compact pass read it. The write re-reads the file
    /// and compares against this before overwriting, so findings another AMF
    /// session appended while the agent ran (or while this dialog sat open) are
    /// re-applied rather than clobbered — the cross-project doc in particular is
    /// shared by every AMF session on the machine.
    pub original_content: String,
    /// Set once a write has been refused because the doc on disk diverged in
    /// ways an append can't explain. The next confirm overwrites deliberately,
    /// so the user is warned but never stuck.
    pub overwrite_confirmed: bool,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎`/`w` write / `e` edit / `esc` discard).
    pub editing: bool,
    /// Scroll offset, in wrapped visual rows, for docs taller than the screen.
    pub scroll: usize,
    /// Request that the next render scroll the cursor back into view. Mirrors
    /// [`FixConfirmState::sync_to_cursor`].
    pub sync_to_cursor: bool,
    /// Last write failure, shown inline so it's recoverable without losing
    /// the edited content.
    pub error: Option<String>,
}

/// Full-screen progress view for the AI PR review's background diff-fetch +
/// review pass (`A`), entered from the AI Review pane.
#[derive(Debug, Clone)]
pub struct AiReviewRunProgress {
    pub stage: crate::app::ai_review::AiReviewStage,
    /// Wall-clock start for a live elapsed timer. This state is intentionally
    /// not persisted; an in-flight headless process belongs to this AMF
    /// process and cannot be resumed after restart.
    pub started_at: std::time::Instant,
    /// Latest sanitized activity label from the harness's structured stream.
    pub activity: Option<String>,
    /// Final token usage, when the harness reports it before completion.
    pub usage: Option<(u64, u64)>,
}

#[derive(Debug, Clone)]
pub struct AiReviewRunState {
    /// The AI Review pane to return to on completion or cancel (dialogs
    /// cleared before stashing, matching the PR Triage `P`/`f` stash
    /// convention).
    pub origin: AiReviewState,
    pub progress: AiReviewRunProgress,
}

/// State for the full-screen AI Review pane — AMF's own review of a PR's
/// diff, independent of PR Triage (see `crate::app::ai_review`'s module doc
/// for why this is a separate workflow rather than bolted onto triage).
#[derive(Debug, Clone)]
pub struct AiReviewState {
    /// Working directory of the feature whose PR this reviews.
    pub workdir: PathBuf,
    /// The PR being reviewed.
    pub pr: crate::github::PrRef,
    /// Findings from the most recent `A` run (or loaded from `ai_review_cache`
    /// on entry), in generation order.
    pub findings: Vec<crate::app::ai_review::AiReviewFinding>,
    /// Overall one-to-three sentence review summary generated in the same
    /// pass as `findings`, and loaded from the same cache row. Older cache
    /// entries may not have one.
    pub summary: Option<String>,
    /// Harness/model/token/cost provenance of the run that produced
    /// `findings`, loaded from the same cache row and refreshed by each `A`
    /// pass. `None` before the first run this SHA, for a legacy cache row, or
    /// when the latest run errored. Surfaced in the pane and attached to the
    /// posted GitHub review.
    pub attribution: Option<crate::app::ai_review::AiReviewAttribution>,
    /// Index into `findings` of the highlighted finding.
    pub selected: usize,
    /// Scroll offset (in lines) for the detail pane of the selected finding.
    pub detail_scroll: usize,
    /// Number of lines the detail pane rendered on the last frame, so the
    /// scroll clamp bounds against what was actually shown.
    pub detail_content_lines: usize,
    /// Record of the most recent `A` run (success/error/finding-count),
    /// shown as a header badge so a review that already ran doesn't look
    /// identical to one that never did.
    pub last_run: Option<crate::app::ai_review::AiReviewRun>,
    /// Harness chosen for this pane's `A` runs, picked once via `harness_pick`
    /// and remembered for the rest of the visit.
    pub harness: Option<AgentKind>,
    /// Single-select picker shown before the first `A` run in this pane.
    pub harness_pick: Option<AiHarnessPickState>,
    /// Harness in effect when the current harness-pick "chain" started —
    /// set the first time this pane's picker steps back from the model
    /// picker to the harness picker, and left untouched by any further
    /// back-and-forth within the same chain (cleared once a review actually
    /// starts). Lets [`App::accept_ai_review_harness_pick`] detect a switch
    /// away from the *original* harness even after the user backs out and
    /// re-confirms an already-switched-to harness, so `AppConfig::review_model`
    /// (which may only be valid for the original harness) isn't reseeded as
    /// an incompatible model for the new one. See `AiHarnessPickState::previous_harness`.
    pub harness_pick_origin: Option<AgentKind>,
    /// Model chosen for this pane's `A` runs, picked once via `model_pick`
    /// right after the harness. `None` means "use the default" — either the
    /// picker hasn't run yet (see `model_picked`) or the user explicitly
    /// chose the "Default" row.
    pub model: Option<String>,
    /// Whether the model has been picked (or auto-skipped, e.g. for Pi) yet
    /// this pane visit.
    pub model_picked: bool,
    /// Single-select picker shown once per pane, right after the harness.
    pub model_pick: Option<AiModelPickState>,
    /// When `Some`, the selected finding's body is open for editing (`e`).
    pub finding_editor: Option<TextEditor>,
    /// When `Some`, the post-to-GitHub confirm dialog is open (`W`).
    pub post_confirm: Option<AiReviewPostConfirmState>,
}

/// State for the full-screen PR Triage pane.
#[derive(Debug, Clone)]
pub struct PrReviewState {
    /// Working directory of the feature whose PR we're reviewing. Used by the
    /// manual-refresh action (`r`) to re-resolve and re-fetch the PR.
    pub workdir: PathBuf,
    /// The fetched, normalized review.
    pub review: crate::app::pr_review::PrReview,
    /// Index into `review.comments` of the highlighted comment.
    pub selected: usize,
    /// Scroll offset (in lines) for the detail pane of the selected comment.
    pub detail_scroll: usize,
    /// Number of lines the detail pane rendered on the last frame. The renderer
    /// (`ui::dialogs::pr_review`) writes this each draw so the scroll clamp can
    /// bound against what was actually shown, rather than a hand-synced estimate
    /// that drifts as the detail layout (Markdown, dividers) changes.
    pub detail_content_lines: usize,
    /// When true, comments already resolved on GitHub are hidden from the list.
    pub hide_resolved: bool,
    /// Order the comment list is shown in (cycled with `o`), independent of
    /// `hide_resolved`.
    pub sort_mode: crate::app::pr_review::PrSortMode,
    /// Which agent session "fix" prompts are injected into. Chosen once, via
    /// `harness_pick`, before the first `f`/`B` of a pane visit.
    pub fix_target: crate::app::pr_review::FixTarget,
    /// Whether `fix_target` (and, for the dedicated case, `review_harness`)
    /// has already been explicitly resolved for this pane visit by the user
    /// confirming `harness_pick`. Prevents re-opening the picker on every
    /// subsequent `f`/`B` while still allowing each new visit to name another
    /// dedicated session.
    pub fix_target_picked: bool,
    /// Token totals already present when each fix-target session joined this
    /// visit to the PR pane. Current totals minus these snapshots are the live
    /// "this visit" tally; a target created after the pane opened has no
    /// baseline, so all of its usage belongs to the visit.
    pub usage_baselines: HashMap<TokenUsageSource, SessionTokenUsage>,
    /// Harness chosen for the dedicated triage session, picked once before the
    /// first fix is injected and reused for the rest of the pane visit. `None`
    /// until the user picks (or when a dedicated session isn't the target).
    /// Lets PR triage run on a different harness than the feature's working
    /// session.
    pub review_harness: Option<AgentKind>,
    /// Label (and lookup identity) of the dedicated triage session selected for
    /// this pane visit. Defaults to `PR Triage` for backwards compatibility,
    /// but can be named before the first `f`/`B` hand-off so several triage
    /// agents can run alongside one another in the same feature.
    pub dedicated_session_label: String,
    /// When `Some`, the fix-target picker is open over the pane: the user is
    /// choosing whether fixes go to the feature's existing live session or a
    /// dedicated triage session (and, for the latter, which harness) before
    /// the first fix/batch is injected. Replaces the old standalone `t`
    /// toggle — the choice is made once, at the point it's needed.
    pub harness_pick: Option<HarnessPickState>,
    /// When `Some`, the compact triage-feature setup overlay is open: the user
    /// picked `New feature…` in the fix-target picker and is choosing the
    /// companion feature's preset / harness / vibe mode before it is created.
    pub new_feature_setup: Option<TriageFeatureSetupState>,
    /// When `Some`, the integration overlay is open: the review of what the
    /// companion triage feature has committed and how to land it on the PR
    /// branch (push, or cherry-pick into the source worktree).
    pub integrate: Option<TriageIntegrateState>,
    /// When `Some`, the fix confirm/edit dialog is open over the pane, holding
    /// the assembled (and editable) prompt awaiting the user's approval before
    /// it is injected into the agent session.
    pub fix_confirm: Option<FixConfirmState>,
    /// Whether the fix confirm/edit dialog opens with the vim keymap. Persisted
    /// on the pane (not the editor, which is rebuilt on each `f`) so the choice
    /// survives reopening the dialog for another comment — the same approach as
    /// [`PlaceholderFillState::vim_enabled`].
    pub fix_vim_enabled: bool,
    /// When `Some`, the reply-kind picker (`R`) is open: choosing between a
    /// "Done" report and a "not needed" explanation before the reply dialog
    /// itself opens.
    pub reply_kind_pick: Option<ReplyKindPickState>,
    /// When `Some`, the "Mark" picker (`m`) is open: choosing Done / Skip /
    /// Resolve-on-GitHub for the selected comment.
    pub mark_pick: Option<MarkPickState>,
    /// When `Some`, the reply dialog is open over the pane: an AI-drafted,
    /// editable reply awaiting the user's approval before it is posted to GitHub.
    pub reply: Option<ReplyState>,
    /// When `Some`, the "add to memory" dialog is open over the pane: the
    /// selected comment's finding, editable, awaiting the user's approval
    /// before it's appended to the review-memory doc.
    pub memory_add: Option<MemoryAddState>,
    /// Comment ids marked (with `space`) for a combined batch fix via `B`.
    /// Keyed by id (not index) so marks survive the hide-resolved filter
    /// shifting the visible rows. Cleared once the batch is injected.
    pub marked: std::collections::HashSet<u64>,
    /// Set while the combined-batch flow (`B`) is waiting on the harness picker:
    /// after the user picks the review harness, the continuation opens the
    /// combined-batch confirm dialog instead of the single-comment one. Cleared
    /// when the picker is confirmed or cancelled.
    pub pending_batch: bool,
    /// The branch actually checked out in `workdir`, snapshotted when the pane
    /// was entered/refreshed (`WorktreeManager::current_branch`). `f`/`B` fix
    /// injection reads files from this workdir regardless of which PR is being
    /// triaged (`G`/`g`/`#` allow picking *any* PR in the repo), so when this
    /// doesn't match `review.pr.head_ref` a fix would silently land on the
    /// wrong branch — see [`Self::branch_mismatch`]. `None` when the branch
    /// couldn't be determined (e.g. detached HEAD).
    pub checked_out_branch: Option<String>,
    /// Completed AI-review findings for this exact PR/head SHA that are still
    /// publishable. Loaded from `ai_review_cache` on entry and kept in sync as
    /// the linked AI Review is generated, skipped, or posted.
    pub pending_ai_review_findings: usize,
    /// Most recent terminal AI Review result for this exact PR/head SHA.
    /// Kept alongside the pending count so a successful zero-finding run or a
    /// failure remains distinguishable from a review that has never run.
    pub ai_review_last_run: Option<crate::app::ai_review::AiReviewRun>,
    /// Persisted read-only investigations for this PR (the `v` → `f` flow),
    /// loaded from `pr_investigations` on entry and the in-memory source of
    /// truth while the overlay is open. Keyed by comment id (one per comment).
    pub investigations: Vec<crate::db::pr_investigations::PrInvestigation>,
    /// When `Some`, the per-run harness picker for a pending investigation is
    /// open over the pane (reuses the flat Learning-Mode `m` picker shape).
    pub investigation_harness_pick: Option<InvestigationHarnessPick>,
    /// When `Some`, the action menu for the selected comment's completed
    /// investigation is open (`a`): convert to fix / add to batch / ask
    /// follow-up / dismiss / keep as TODO.
    pub investigation_action_pick: Option<InvestigationActionPick>,
    /// When `Some`, the follow-up question editor is open for one investigation
    /// (Learning Mode `F` behaviour — the answer re-runs headless with the
    /// prior turn as context).
    pub investigation_follow_up: Option<InvestigationFollowUpDraft>,
    /// A submitted follow-up question waiting on the harness picker, so
    /// [`crate::app::App::pr_review_investigation_harness_confirm`] knows the
    /// run is a follow-up (not a fresh investigation) and for which comment.
    pub pending_follow_up: Option<PendingFollowUp>,
    /// Optional free-form context the operator attaches to the *next* fresh
    /// Investigate run (`e`): a hypothesis for the read-only agent to verify
    /// against the PR and repo, not a fact to assume. Persists across the pane
    /// visit until an investigation consumes it; empty = today's behaviour.
    pub investigation_context: InvestigationContextField,
}

/// The optional investigation-context note plus its inline edit box. Bundled
/// into one field so the many `PrReviewState` literals only grow by a line.
#[derive(Debug, Clone, Default)]
pub struct InvestigationContextField {
    /// The committed note. Empty means no note was attached.
    pub note: String,
    /// The inline editor, `Some` only while focused (`e`). Seeded from `note`
    /// on open; `Enter` commits its text back to `note`, `Esc` discards.
    pub editor: Option<crate::editor::TextEditor>,
}

/// One row of the completed-investigation action menu (`a`). Fix and batch
/// are deliberately absent — `f` and `B` act on a comment normally whether or
/// not it has an investigation; the findings just stay visible in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvestigationAction {
    /// Open an editable reply draft prefilled from the answer; on approve it
    /// posts via the existing `gh` reply path and marks the comment `Replied`.
    PostReply,
    /// Re-run headless with the prior question+answer as context.
    AskFollowUp,
    /// Mark the finding handled without acting on it.
    Dismiss,
    /// Write the finding to a scoped TODO list.
    KeepAsTodo,
}

impl InvestigationAction {
    pub const ALL: [InvestigationAction; 4] = [
        InvestigationAction::PostReply,
        InvestigationAction::AskFollowUp,
        InvestigationAction::Dismiss,
        InvestigationAction::KeepAsTodo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InvestigationAction::PostReply => "Post a reply (editable draft)",
            InvestigationAction::AskFollowUp => "Ask a follow-up",
            InvestigationAction::Dismiss => "Dismiss the finding",
            InvestigationAction::KeepAsTodo => "Keep as a TODO",
        }
    }
}

/// The completed-investigation action menu (`a`), a sub-state of
/// [`PrReviewState`] like the other PR-triage pickers.
#[derive(Debug, Clone)]
pub struct InvestigationActionPick {
    /// The comment whose investigation this menu acts on (re-resolved on
    /// confirm, since a background refresh can move the selection).
    pub comment_id: u64,
    pub selected: usize,
}

/// The inline follow-up question editor for one investigation.
#[derive(Debug, Clone)]
pub struct InvestigationFollowUpDraft {
    pub comment_id: u64,
    pub editor: crate::editor::TextEditor,
}

/// A submitted follow-up question, parked while the harness picker is open.
#[derive(Debug, Clone)]
pub struct PendingFollowUp {
    pub comment_id: u64,
    pub question: String,
}

/// Flat single-select harness picker shown before an investigation runs — the
/// operator picks the harness per investigation. Mirrors
/// [`crate::app::LearningHarnessPicker`].
#[derive(Debug, Clone)]
pub struct InvestigationHarnessPick {
    pub harnesses: Vec<crate::project::AgentKind>,
    pub selected: usize,
}

/// Transient state while a blocking read-only investigation runs off the UI
/// thread. Holds the PR-triage pane to restore when the run returns, so the
/// overlay is modal for exactly the duration of the call (the deliberate
/// contrast with Learning Mode's non-blocking queue).
#[derive(Debug, Clone)]
pub struct PrInvestigationLoadState {
    /// The pane to return to once the run finishes (or is cancelled).
    pub review: Box<PrReviewState>,
    /// The comment being investigated.
    pub comment_id: u64,
    /// The harness the operator picked for this run.
    pub harness: crate::project::AgentKind,
    /// Start of the run, for the loading frame's elapsed-time line.
    pub started_at: std::time::Instant,
    pub pr_number: u32,
    pub pr_url: String,
}

/// Identity of the PR Triage refresh started after a successful AI Review
/// post. Kept outside `AppMode` so the refresh can update a stashed triage
/// pane while the user remains in AI Review.
#[derive(Debug, Clone)]
pub struct AiReviewTriageRefresh {
    pub workdir: PathBuf,
    pub pr: crate::github::PrRef,
}

/// Confirm/edit dialog for posting the kept AI-review findings to GitHub as a
/// real review (`W`). Built once from every not-skipped, not-yet-published
/// finding; `⏎` posts as-is. Only the summary body is editable — the
/// per-finding inline comment bodies are the AI's own text, vetted by
/// skipping (`s`) rather than hand-edited here.
#[derive(Debug, Clone)]
pub struct AiReviewPostConfirmState {
    /// Inline review comments built from the anchored findings.
    pub inline: Vec<crate::github::PrReviewComment>,
    pub editor: TextEditor,
    pub editing: bool,
    /// Last post failure, shown inline so a recoverable error (e.g. GitHub
    /// rejecting the review because a finding no longer matches the current
    /// diff) doesn't require leaving the dialog to notice — `show_error`
    /// unconditionally resets `self.mode` to `Normal` outside of
    /// `Normal`/`Help`/`Viewing`, so the pane is restored with this set
    /// rather than losing the dialog entirely.
    pub error: Option<String>,
}

/// A [`PrReviewState`] stashed while the user is watching the linked fix
/// session (`P` from PR Triage), so `leader+P` can jump straight back
/// to the exact comment/scroll/dialog state without re-fetching. `session`
/// and `window` identify the tmux target the stash was jumped *to*, so the
/// restore only fires from that same session's view — a stash left behind
/// after navigating elsewhere is not mistaken for a different PR's pane.
#[derive(Debug, Clone)]
pub struct PrReviewReturn {
    pub session: String,
    pub window: String,
    pub state: PrReviewState,
}

/// One row of the final-review destination picker
/// ([`ReviewDestinationPickState`]). Mirrors PR Triage's
/// [`crate::app::pr_review::FixTargetPickRow`] but adds a row per *other*
/// feature in the store, so a review's fixes can be routed into an unrelated
/// feature's agent session.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewDestinationRow {
    /// The reviewed feature's own first agent session (carries its label when
    /// one exists, so the picker names exactly where the prompt lands).
    ExistingLive(Option<String>),
    /// A fresh dedicated "Final Review" session on this harness, in the
    /// reviewed feature.
    Dedicated(crate::project::AgentKind),
    /// Another feature that already exists in the store — its first agent
    /// session receives the prompt.
    ExistingFeature { feature_id: String, label: String },
    /// Create a brand-new companion feature (opens the compact setup overlay).
    NewFeature,
}

impl ReviewDestinationRow {
    pub fn label(&self) -> String {
        match self {
            ReviewDestinationRow::ExistingLive(Some(name)) => {
                format!("This feature's live session ({name})")
            }
            ReviewDestinationRow::ExistingLive(None) => "This feature's live session".to_string(),
            ReviewDestinationRow::Dedicated(agent) => {
                format!("Dedicated review session ({})", agent.display_name())
            }
            ReviewDestinationRow::ExistingFeature { label, .. } => {
                format!("Another feature: {label}")
            }
            ReviewDestinationRow::NewFeature => {
                "New feature… (isolated worktree, own harness + mode)".to_string()
            }
        }
    }
}

/// The final-review destination picker, opened with `t` in the review viewer
/// and held as a sub-state of [`DiffViewerState`] (mirroring PR Triage's
/// `harness_pick` on `PrReviewState`). One modal list; choosing a row resolves
/// `DiffViewerState.fix_target` (+ `review_harness` / `fix_target_feature_id`),
/// except `NewFeature`, which opens `review_feature_setup`.
#[derive(Debug, Clone)]
pub struct ReviewDestinationPickState {
    pub rows: Vec<ReviewDestinationRow>,
    pub selected: usize,
}

/// Single-select fix-target picker shown before the first fix/batch of a PR
/// Triage pane visit: whether fixes go to the feature's existing live
/// session, or a dedicated triage session pinned to a specific harness.
/// Replaces the old standalone `t` toggle — the choice is made once, at the
/// point it's needed, instead of living as an always-on key. Highlights the
/// dedicated-review row for the project's preferred agent by default.
#[derive(Debug, Clone)]
pub struct HarnessPickState {
    /// The rows to choose from: the existing-live option, plus one row per
    /// allowed agent for a dedicated session.
    pub rows: Vec<crate::app::pr_review::FixTargetPickRow>,
    /// Index into `rows` of the highlighted choice.
    pub selected: usize,
    /// `Some` after a dedicated harness row is chosen, while the picker is on
    /// its second step accepting an optional session name. An empty name means
    /// the backwards-compatible `PR Triage` label.
    pub session_name: Option<String>,
}

/// One editable row of the compact triage-feature setup overlay
/// ([`TriageFeatureSetupState`]). Deliberately much smaller than the full
/// feature-creation wizard: only the settings that change how the *triage*
/// agent behaves, plus the branch it lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageSetupRow {
    /// Apply a configured feature preset (or "Manual", which changes nothing).
    Preset,
    /// Which agent harness the triage feature runs.
    Harness,
    /// Vibe mode — the setting the whole feature exists for: triaging review
    /// comments in, say, Vibeless while the source feature runs SuperVibe.
    Mode,
    /// Review mode (developer notes on every change).
    Review,
    /// Chrome/browser automation.
    Chrome,
    /// The companion branch name. Pre-filled and editable.
    Branch,
}

impl TriageSetupRow {
    pub const ALL: [TriageSetupRow; 6] = [
        TriageSetupRow::Preset,
        TriageSetupRow::Harness,
        TriageSetupRow::Mode,
        TriageSetupRow::Review,
        TriageSetupRow::Chrome,
        TriageSetupRow::Branch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriageSetupRow::Preset => "Preset",
            TriageSetupRow::Harness => "Harness",
            TriageSetupRow::Mode => "Vibe mode",
            TriageSetupRow::Review => "Review mode",
            TriageSetupRow::Chrome => "Chrome",
            TriageSetupRow::Branch => "Branch",
        }
    }
}

/// The compact feature-creation flow shown when the user picks `New feature…`
/// as the fix target: a single settings list (no multi-step wizard) that
/// creates an isolated, worktree-backed companion feature for this PR's
/// triage work.
///
/// Plan mode is deliberately absent — it defers the launch into a planning
/// interview, which makes no sense for a feature whose whole job is to apply
/// review comments that already say what to do.
#[derive(Debug, Clone)]
pub struct TriageFeatureSetupState {
    /// Presets available for this repo. Index 0 of the *choice* is "Manual"
    /// (no preset); `presets[i - 1]` for any higher index.
    pub presets: Vec<crate::extension::FeaturePreset>,
    pub preset_index: usize,
    /// Harnesses allowed for this repo.
    pub agents: Vec<AgentKind>,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
    /// Companion branch name — deliberately *not* the PR's branch, which git
    /// can't check out in a second worktree.
    pub branch: String,
    /// Focused row.
    pub row: usize,
    /// Inline validation/creation error (e.g. a duplicate feature name), shown
    /// in the overlay so the user can correct it without losing the pane.
    pub error: Option<String>,
    /// True when the combined-batch flow (`B`) opened this, so the
    /// continuation after creation reopens the batch dialog rather than the
    /// single-comment one — mirroring `PrReviewState::pending_batch`.
    pub pending_batch: bool,
}

impl TriageFeatureSetupState {
    /// The chosen preset, or `None` for "Manual".
    pub fn selected_preset(&self) -> Option<&crate::extension::FeaturePreset> {
        self.preset_index
            .checked_sub(1)
            .and_then(|i| self.presets.get(i))
    }

    /// Display text for the preset row.
    pub fn preset_label(&self) -> String {
        match self.selected_preset() {
            Some(preset) => preset.name.clone(),
            None => "Manual".to_string(),
        }
    }

    /// The focused row, or `Branch` if `row` somehow ran past the list.
    pub fn focused_row(&self) -> TriageSetupRow {
        TriageSetupRow::ALL
            .get(self.row)
            .copied()
            .unwrap_or(TriageSetupRow::Branch)
    }

    pub fn agent(&self) -> AgentKind {
        self.agents
            .get(self.agent_index)
            .cloned()
            .unwrap_or_default()
    }
}

/// How the companion triage feature's commits get back onto the PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageIntegration {
    /// `git push <remote> <triage-branch>:<pr-branch>` — a normal
    /// fast-forward push. Never forced: a diverged PR branch is reported, not
    /// overwritten.
    Push,
    /// Cherry-pick the triage commits into the source worktree. Offered only
    /// when that worktree is clean, so an in-progress change is never
    /// clobbered.
    CherryPick,
}

impl TriageIntegration {
    pub const ALL: [TriageIntegration; 2] =
        [TriageIntegration::Push, TriageIntegration::CherryPick];

    pub fn label(self) -> &'static str {
        match self {
            TriageIntegration::Push => "Push to the PR branch",
            TriageIntegration::CherryPick => "Cherry-pick into the source worktree",
        }
    }
}

/// The integration overlay (`I`): what the companion triage feature has
/// committed since it branched, and the two explicit, non-destructive ways to
/// land it on the PR. Everything here is computed before the overlay opens, so
/// the user sees exactly what will happen before confirming.
#[derive(Debug, Clone)]
pub struct TriageIntegrateState {
    /// Companion branch holding the triage commits.
    pub triage_branch: String,
    /// The PR's own head branch — where a push lands. Not necessarily the
    /// branch the source worktree has checked out.
    pub pr_branch: String,
    /// One-line summaries of the commits on the triage branch since it
    /// branched (newest first), for the "what will land" preview.
    pub commits: Vec<String>,
    /// Set when the source worktree has uncommitted changes: the cherry-pick
    /// option is disabled and this explains why. Pushing is unaffected — it
    /// never touches the source worktree.
    pub source_dirty: Option<String>,
    /// Set when the companion worktree itself has uncommitted changes — those
    /// wouldn't be included, so say so rather than silently landing less than
    /// the user expects.
    pub triage_dirty: bool,
    pub selected: usize,
    /// Inline result/error from the last attempt, kept in the overlay so a
    /// rejected push can be read and retried in place.
    pub error: Option<String>,
    /// Set once an integration succeeded, so the overlay reports the outcome
    /// instead of inviting the same action again.
    pub done: Option<String>,
    /// Companion feature id captured when the overlay opened. The final-review
    /// companion flow (`AppMode::ReviewIntegrate`) sets this so the confirm
    /// step re-resolves the exact feature by id rather than by a branch-name
    /// scan, which can collide across projects (`<branch>-review-fixes` is only
    /// unique within its repo). PR Triage leaves it `None` — it re-resolves
    /// from PR context instead.
    pub companion_feature_id: Option<String>,
}

impl TriageIntegrateState {
    pub fn focused(&self) -> TriageIntegration {
        TriageIntegration::ALL
            .get(self.selected)
            .copied()
            .unwrap_or(TriageIntegration::Push)
    }
}

/// Harness picker for the paid, headless `A` review pass. An unavailable CLI
/// leaves the picker open and records an actionable inline error.
#[derive(Debug, Clone)]
pub struct AiHarnessPickState {
    pub agents: Vec<AgentKind>,
    pub selected: usize,
    pub error: Option<String>,
    /// The harness-pick chain's original harness (`AiReviewState::harness_pick_origin`),
    /// carried into this picker so a confirm can tell whether the choice has
    /// actually diverged from where the chain started — not just from the
    /// harness shown on the immediately preceding screen. `None` on the
    /// initial harness step. Used to avoid seeding one harness's model choice
    /// (or the globally configured default model) into a different harness's
    /// rebuilt model picker.
    pub previous_harness: Option<AgentKind>,
}

/// One row of the model picker: either "use the default", a known-good
/// preset `--model` value, or "type your own".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPickRow {
    /// No explicit model — the harness's own default (or `AppConfig::review_model`
    /// as an underlying override) applies.
    Default,
    /// A verified alias/name for the chosen harness (e.g. Claude's `"sonnet"`,
    /// or one of the account's own model ids reported by
    /// `codex_config::known_models`). Owned rather than `&'static str` since
    /// Codex's come from a file read at runtime, not a fixed literal list.
    Preset(String),
    /// Free-text entry for anything not in the preset list.
    Custom,
}

/// Single-select model picker for the `A` AI review, shown once per pane
/// right after the harness is chosen. Presets are a best-effort, *verified*
/// set of model names for the chosen harness: Claude's four well-known tier
/// aliases (`sonnet`/`opus`/`haiku`/`fable`, confirmed against `claude
/// --help`), or Codex's account-specific model ids read from
/// `~/.codex/config.toml` (`codex_config::known_models`). Other harnesses
/// offer just `Default` and `Custom`, since their valid model strings aren't
/// reliably enumerable here; guessing wrong presets would be worse than not
/// offering any.
#[derive(Debug, Clone)]
pub struct AiModelPickState {
    pub rows: Vec<ModelPickRow>,
    /// Index into `rows` of the highlighted choice.
    pub selected: usize,
    /// Free-text buffer for the `Custom` row, live only while `editing_custom`.
    pub custom_input: String,
    /// True while keystrokes go to `custom_input` (opened by `⏎`/`e` on the
    /// `Custom` row); false in the plain list-navigation view.
    pub editing_custom: bool,
}

/// Single-select picker shown by `R` before the reply dialog itself: choose
/// between a "Done in `<sha>`" report and a "not needed" explanation. Once
/// confirmed, routes into the same [`ReplyState`] flow either kind already
/// used — this is purely a UI step in front of it, replacing the old
/// separate `R`/`n` top-level keys.
#[derive(Debug, Clone)]
pub struct ReplyKindPickState {
    /// Index into `ReplyKind::ALL` of the highlighted choice.
    pub selected: usize,
}

/// Single-select picker shown by `m` ("Mark"): choose between marking the
/// selected comment `Done` (local), `Skip` (local), or toggling its GitHub
/// review thread's resolved state. Replaces the old separate `m`/`s`/`x`
/// top-level keys with one entry point; applying a row is immediate (no
/// further confirm step, matching the original single-key behavior) since
/// none of the three actions need editable text.
#[derive(Debug, Clone)]
pub struct MarkPickState {
    /// Index into `MarkAction::ALL` of the highlighted choice.
    pub selected: usize,
}

/// Reply dialog for one comment. Replies are contextual, not free-form: either
/// a "Done in `<sha>`." report of a completed fix or a "not needed" explanation.
/// The seeded body is editable; nothing is posted until the user confirms.
#[derive(Debug, Clone)]
pub struct ReplyState {
    /// GitHub id of the comment being replied to (resolves the post target).
    pub comment_id: u64,
    /// Which contextual reply this is (drives the seed, title, and the triage
    /// outcome applied on post).
    pub kind: crate::app::pr_review::ReplyKind,
    /// The reply body, editable before posting.
    pub editor: TextEditor,
    /// Whether the initial body came back from an agent fix session. Agent
    /// drafts receive AI-authorship attribution; deterministic templates and
    /// user-written not-needed replies receive channel-only AMF attribution.
    /// Only ever `true` for [`crate::app::pr_review::ReplyKind::Done`] — see
    /// [`crate::app::pr_review::App::open_reply`].
    pub agent_drafted: bool,
    /// Best-effort details about the agent session that produced the draft.
    /// Captured when the reply opens so the confirmation UI previews the exact
    /// disclosure that will be posted with an unchanged AI-authored reply.
    pub generation_metadata: Option<crate::app::pr_review::ReplyGenerationMetadata>,
    /// The exact body the editor was seeded with when the dialog opened.
    /// Compared against the current editor text at post time: if the user has
    /// changed it, the draft is no longer purely the agent's own words, so
    /// `agent_drafted` attribution no longer applies (see
    /// [`crate::app::pr_review::reply_effective_agent_drafted`]).
    pub original_seed: String,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎` post / `e` edit / `esc` cancel).
    pub editing: bool,
}

/// "Add to memory" dialog (`M`): appends the selected comment's distilled
/// finding to the review-findings memory doc
/// (`review_memory::append_finding`). Mirrors [`ReplyState`]'s edit/confirm
/// split, plus a category cycled with `Tab` in the confirm view.
#[derive(Debug, Clone)]
pub struct MemoryAddState {
    /// GitHub id of the comment the finding is drawn from.
    pub comment_id: u64,
    /// Index into `crate::app::pr_review::MEMORY_CATEGORIES`, cycled with `Tab`.
    pub category: usize,
    /// Which doc the finding lands in, toggled with `g`. Defaults to
    /// `Project` — a finding from this PR is about this repo until the user
    /// says it's a habit worth carrying everywhere.
    pub scope: crate::app::review_memory::MemoryScope,
    /// The finding text, editable before it's appended.
    pub editor: TextEditor,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎` append / `e` edit / `Tab` cycle category / `g` toggle
    /// scope / `esc` cancel).
    pub editing: bool,
}

/// Confirm/edit dialog for a fix prompt: shows the exact text that will be
/// injected (token principle #3 — no file contents), with a `~N tokens`
/// preview, before it reaches the agent. The prompt is editable so the user can
/// tweak it before sending.
#[derive(Debug, Clone)]
pub struct FixConfirmState {
    /// The assembled fix prompt, editable before injection.
    pub editor: TextEditor,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// default confirm view (`⏎` inject / `e` edit / `esc` cancel).
    pub editing: bool,
    /// Scroll offset, in wrapped visual rows, for prompts taller than the
    /// dialog. Clamped to the rendered content each frame.
    pub scroll: usize,
    /// Request that the next render scroll the cursor back into view. Set on
    /// edits / cursor moves and cleared once applied; an explicit scroll key
    /// clears it so the user can scroll away from the cursor.
    pub sync_to_cursor: bool,
    /// When `Some`, this dialog holds a **combined** batch prompt built from
    /// several marked comments (the `B` flow) rather than a single comment's
    /// fix. The vector is the ids of every comment included in the batch;
    /// injecting marks all of them `Fixing` and clears the marked set. `None`
    /// for an ordinary single-comment fix (only the selected comment is marked).
    pub batch: Option<Vec<u64>>,
    /// Per-comment correlation ids embedded in the prompt's `amf reply-draft`
    /// handoff commands. They become authoritative only when the user confirms
    /// injection, at which point AMF invalidates any older stored draft.
    pub reply_draft_requests: Vec<crate::app::pr_review::ReplyDraftRequest>,
}

impl PrReviewState {
    pub fn selected_comment(&self) -> Option<&crate::app::pr_review::PrComment> {
        self.review.comments.get(self.selected)
    }

    /// The checked-out branch when it's known and doesn't match the PR being
    /// triaged — `None` when they match, or either side is unknown (an empty
    /// `head_ref` means the PR was resolved before this field existed; a
    /// `None` `checked_out_branch` means detached HEAD or the branch lookup
    /// failed). Surfaced as a pane-header warning and inside the fix confirm
    /// dialog, since fix injection reads files from `workdir` regardless of
    /// which PR is loaded.
    pub fn branch_mismatch(&self) -> Option<&str> {
        branch_mismatch(&self.review.pr.head_ref, self.checked_out_branch.as_deref())
    }

    /// Indices into `review.comments` that pass the current filter, ordered by
    /// `sort_mode`. With `hide_resolved` on, GitHub-resolved comments are
    /// dropped. AMF follow-up replies whose root is present are always
    /// collated under that root's detail view instead of duplicated here.
    pub fn visible_indices(&self) -> Vec<usize> {
        let indices = self
            .review
            .comments
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                !self.review.is_collated_amf_reply(c) && (!self.hide_resolved || !c.is_resolved)
            })
            .map(|(i, _)| i)
            .collect();
        self.sort_indices(indices)
    }

    /// Every comment index — including ones `visible_indices` would drop for
    /// `hide_resolved` or collation — in `sort_mode` order. Used to find a
    /// hidden selection's nearest visible neighbor when a filter or refresh
    /// hides it. Must stay unfiltered so `self.selected` itself can always be
    /// located by `position()`, even when `selected` is the very comment that
    /// just became hidden (e.g. an orphaned AMF reply that a refresh just
    /// collated under its now-present root); the neighbor search then walks
    /// this order and tests `visible_indices().contains()` to find the
    /// nearest comment that's actually shown.
    pub(crate) fn all_sorted_indices(&self) -> Vec<usize> {
        self.sort_indices((0..self.review.comments.len()).collect())
    }

    /// If `selected` is currently hidden by `hide_resolved`, snap it to the
    /// nearest remaining visible comment in sort order (forward first, then
    /// backward, then the first visible comment). No-op when `selected` is
    /// already visible, or nothing is visible at all. Shared by the `x`
    /// toggle and by a PR Triage refresh, either of which can newly hide the
    /// selected comment (resolved on GitHub, in the toggle case; refreshed
    /// into a resolved state, in the refresh case).
    pub fn snap_selection_to_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() || visible.contains(&self.selected) {
            return;
        }
        let order = self.all_sorted_indices();
        let pos = order.iter().position(|&i| i == self.selected);
        let snapped = pos
            .and_then(|p| order[p..].iter().find(|i| visible.contains(i)))
            .or_else(|| pos.and_then(|p| order[..p].iter().rev().find(|i| visible.contains(i))))
            .copied()
            .unwrap_or(visible[0]);
        self.selected = snapped;
        self.detail_scroll = 0;
    }

    /// Apply `sort_mode` to a set of comment indices. Stable, so ties keep
    /// their relative (fetch) order.
    fn sort_indices(&self, mut indices: Vec<usize>) -> Vec<usize> {
        use crate::app::pr_review::PrSortMode;
        match self.sort_mode {
            PrSortMode::FetchOrder => {}
            PrSortMode::ByFile => indices.sort_by(|&a, &b| {
                let path = |i: usize| self.review.comments[i].path.as_deref();
                match (path(a), path(b)) {
                    (Some(pa), Some(pb)) => pa.cmp(pb),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            }),
            PrSortMode::ByAuthor => indices.sort_by(|&a, &b| {
                self.review.comments[a]
                    .author
                    .cmp(&self.review.comments[b].author)
            }),
            PrSortMode::HumansFirst => indices.sort_by_key(|&i| self.review.comments[i].is_bot),
            PrSortMode::Conversations => indices.sort_by_key(|&i| {
                matches!(
                    self.review.comments[i].kind,
                    crate::app::pr_review::CommentKind::Conversation
                )
            }),
        }
        indices
    }

    /// Under [`PrSortMode::Conversations`], the position within
    /// [`Self::visible_indices`] where the conversation-comment section
    /// begins — `None` when not in that mode, or when the visible list has no
    /// conversation comments (nothing to separate) or is *entirely*
    /// conversation comments (no code-anchored section to divide from).
    /// `draw_comment_list` uses this to insert a section divider rather than
    /// silently reordering the list.
    pub fn conversation_section_start(&self) -> Option<usize> {
        if self.sort_mode != crate::app::pr_review::PrSortMode::Conversations {
            return None;
        }
        let visible = self.visible_indices();
        let start = visible.iter().position(|&i| {
            matches!(
                self.review.comments[i].kind,
                crate::app::pr_review::CommentKind::Conversation
            )
        })?;
        (start > 0).then_some(start)
    }

    /// Number of comments hidden by the resolved filter (0 when showing all).
    pub fn hidden_resolved_count(&self) -> usize {
        if !self.hide_resolved {
            return 0;
        }
        self.review
            .comments
            .iter()
            .filter(|c| c.is_resolved)
            .count()
    }
}

/// Free function behind [`PrReviewState::branch_mismatch`] — pulled out so it's
/// testable without constructing a full `PrReviewState`. `None` when the
/// branches match, or when either side is unknown (empty `head_ref` from a
/// pre-existing cache row, or no `checked_out_branch` — detached HEAD / lookup
/// failure).
fn branch_mismatch<'a>(pr_head_ref: &str, checked_out_branch: Option<&'a str>) -> Option<&'a str> {
    let checked_out = checked_out_branch?;
    if pr_head_ref.is_empty() || pr_head_ref == checked_out {
        return None;
    }
    Some(checked_out)
}
#[cfg(test)]
mod tests {
    use super::branch_mismatch;
    #[test]
    fn branch_mismatch_none_when_branches_match() {
        assert_eq!(branch_mismatch("main", Some("main")), None);
    }

    #[test]
    fn branch_mismatch_some_when_branches_differ() {
        assert_eq!(
            branch_mismatch("main", Some("other-branch")),
            Some("other-branch")
        );
    }

    #[test]
    fn branch_mismatch_none_when_pr_head_ref_unknown() {
        // Pre-existing cache row from before `head_ref` existed.
        assert_eq!(branch_mismatch("", Some("other-branch")), None);
    }

    #[test]
    fn branch_mismatch_none_when_checked_out_branch_unknown() {
        // Detached HEAD or the `git branch --show-current` lookup failed.
        assert_eq!(branch_mismatch("main", None), None);
    }
}
