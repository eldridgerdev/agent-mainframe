use super::{estimate_tokens, strip_bot_boilerplate};
use crate::app::review_memory;
use crate::app::review_memory::MemoryScope;
use crate::app::{
    AgentKind, App, AppMode, BootstrapPickState, BootstrapRunState, CompactConfirmState,
    CompactReviewState, CompactRunState, MemoryAddState, ReviewAction,
};
use crate::editor::TextEditor;
use crate::github::{GhCli, PrListEntry, Review, ReviewComment};
use crate::headless::HeadlessRunner;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Categories offered in the "add to memory" dialog (`Tab` cycles), matching
/// the examples in the review-memory doc's own header template. `General` is
/// the default and also what a blank category falls back to in
/// `review_memory::append_finding`.
pub(crate) const MEMORY_CATEGORIES: &[&str] = &[
    "General",
    "Concurrency",
    "Error handling",
    "Naming",
    "Tests",
    "Performance",
    "API design",
    "Style",
];

/// Practical ceiling for the "All" lookback depth. Not truly unbounded — a
/// repo's full closed-PR history could be thousands deep, and both the `gh`
/// fetch loop and the one-shot distill pass scale with it.
pub(super) const BOOTSTRAP_ALL_LIMIT: u32 = 500;

/// How far back the review-memory lookback bootstrap (Epic E) looks when
/// seeding `review-memory.md` from history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapDepth {
    Twenty,
    Fifty,
    Hundred,
    All,
}

impl BootstrapDepth {
    pub const ALL: [BootstrapDepth; 4] = [Self::Twenty, Self::Fifty, Self::Hundred, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Twenty => "20 PRs",
            Self::Fifty => "50 PRs",
            Self::Hundred => "100 PRs",
            Self::All => "All",
        }
    }

    /// The `gh pr list --limit` value this depth fetches.
    pub fn limit(self) -> u32 {
        match self {
            Self::Twenty => 20,
            Self::Fifty => 50,
            Self::Hundred => 100,
            Self::All => BOOTSTRAP_ALL_LIMIT,
        }
    }
}

impl Default for BootstrapDepth {
    /// Matches the plan's mockup, which highlights 50 PRs by default.
    fn default() -> Self {
        Self::Fifty
    }
}

/// Progress of the background lookback-bootstrap fetch + distill (`b` in the
/// PR picker). Two stages: the `gh` fetch loop (zero agent tokens) and the one
/// headless agent pass that clusters the gathered comments into findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStage {
    FetchingComments,
    Distilling {
        pr_count: usize,
        token_estimate: usize,
    },
}

/// Stage of the review-memory compact pass's full-screen running view
/// (Epic E "prevent review-memory rot"). Mirrors [`BootstrapStage`]: a cheap
/// prep stage (reading the doc off disk), then the one paid pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactStage {
    ReadingDoc,
    Compacting { token_estimate: usize },
}

/// Outcome of a completed compact run: the doc was read, rewritten by one
/// headless agent pass, and the proposed replacement is awaiting the user's
/// review before anything is written to disk.
#[derive(Debug, Clone)]
pub struct CompactOutcome {
    /// Bullet count in the doc as it stood before compacting.
    pub original_findings: usize,
    /// Bullet count in the agent's proposed replacement.
    pub proposed_findings: usize,
    /// The full proposed replacement document text.
    pub proposed_content: String,
    /// The doc exactly as it was read off disk for this pass. Carried so the
    /// write can tell whether another session appended to the file while the
    /// agent ran or the user reviewed the proposal, instead of overwriting
    /// findings it never saw (see [`review_memory::doc_drift`]).
    pub original_content: String,
}

/// Messages sent back from the background compact thread. `Compacting` fires
/// once, right before the one headless agent call, so the running screen can
/// show a token estimate; `Done` fires exactly once at the end. `Ok(None)`
/// means there was nothing to compact (doc missing or has zero findings) —
/// distinct from an error, since it isn't one.
pub enum CompactProgress {
    Compacting { token_estimate: usize },
    Done(Result<Option<CompactOutcome>>),
}

/// Outcome of a completed bootstrap run.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapOutcome {
    /// PRs whose comments/reviews contributed non-empty text to the prompt.
    pub pr_count: usize,
    /// Findings newly appended to the memory doc (dedup-aware — re-running
    /// the bootstrap over overlapping history won't double them up).
    pub appended: usize,
}

/// Messages sent back from the background bootstrap thread. `Distilling` fires
/// once, right before the one headless agent call, so the running screen can
/// show a token estimate for that call; `Done` fires exactly once at the end.
pub enum BootstrapProgress {
    Distilling {
        pr_count: usize,
        token_estimate: usize,
    },
    Done(Result<BootstrapOutcome>),
}

/// Flatten one PR's review comments + review summaries into plain-text lines
/// for the lookback-bootstrap prompt (Epic E). Bot bodies are stripped like
/// everywhere else in this module; empty bodies (bare approvals, blank
/// comments) are dropped. Returns an empty string when the PR has nothing
/// worth feeding to the distiller.
pub(super) fn bootstrap_pr_text(comments: &[ReviewComment], reviews: &[Review]) -> String {
    let mut lines = Vec::new();
    for c in comments {
        let text = if c.user.is_bot() {
            strip_bot_boilerplate(&c.body)
        } else {
            c.body.clone()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let loc = match &c.path {
            Some(p) => match c.line.or(c.original_line) {
                Some(l) => format!("{p}:{l}"),
                None => p.clone(),
            },
            None => "general".to_string(),
        };
        lines.push(format!("- ({loc}) {}", text.replace('\n', " ")));
    }
    for r in reviews {
        let text = if r.user.is_bot() {
            strip_bot_boilerplate(&r.body)
        } else {
            r.body.clone()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        lines.push(format!("- (review) {}", text.replace('\n', " ")));
    }
    lines.join("\n")
}

/// Assemble the one distill prompt from every PR's gathered text. Instructs
/// the agent to output the same `## Category` / `- bullet` shape
/// [`review_memory::append_finding`] writes, so the response can be fed
/// straight back through [`review_memory::parse_findings_markdown`] with no
/// further parsing.
/// The `{{pr_history}}` value: one `### PR #n: title` block per gathered PR,
/// trimmed at the end (the built-in template already ends with `---`).
pub(super) fn bootstrap_pr_history(pr_bodies: &[(u32, String, String)]) -> String {
    let mut out = String::new();
    for (number, title, body) in pr_bodies {
        out.push_str(&format!("### PR #{number}: {title}\n{body}\n\n"));
    }
    out.trim_end().to_string()
}

/// The full built-in review-memory bootstrap prompt. Production renders the
/// resolved template ([`run_review_memory_bootstrap`]); kept whole for tests.
#[allow(dead_code)]
pub(super) fn bootstrap_prompt(pr_bodies: &[(u32, String, String)]) -> String {
    crate::prompts::render_template(
        crate::prompts::PromptId::ReviewMemoryBootstrap
            .spec()
            .default_template,
        &crate::prompts::PromptContext::new().with("pr_history", bootstrap_pr_history(pr_bodies)),
    )
}

/// Background body of the lookback bootstrap (Epic E): fetch comments/reviews
/// for every listed PR (zero agent tokens), then make **one** headless agent
/// pass to cluster them into findings and append the new ones to the memory
/// doc. Runs off the UI thread; progress and the final result are reported
/// over `tx`. A single PR's fetch failure is skipped rather than aborting the
/// whole run — one stale/deleted PR shouldn't sink the batch.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_review_memory_bootstrap(
    workdir: PathBuf,
    memory_path: PathBuf,
    memory_scope: MemoryScope,
    entries: Vec<PrListEntry>,
    model: Option<String>,
    // The resolved `review_memory.bootstrap` template (built-in or override),
    // rendered here once the PR history is gathered on this worker thread.
    template: String,
    tx: std::sync::mpsc::Sender<BootstrapProgress>,
) {
    let mut pr_bodies = Vec::new();
    for entry in &entries {
        let comments = GhCli::pr_review_comments(&workdir, entry.number).unwrap_or_default();
        let reviews = GhCli::pr_reviews(&workdir, entry.number).unwrap_or_default();
        let text = bootstrap_pr_text(&comments, &reviews);
        if !text.is_empty() {
            pr_bodies.push((entry.number, entry.title.clone(), text));
        }
    }

    if pr_bodies.is_empty() {
        let _ = tx.send(BootstrapProgress::Done(Ok(BootstrapOutcome {
            pr_count: 0,
            appended: 0,
        })));
        return;
    }

    let prompt = crate::prompts::render_template(
        &template,
        &crate::prompts::PromptContext::new().with("pr_history", bootstrap_pr_history(&pr_bodies)),
    );
    let _ = tx.send(BootstrapProgress::Distilling {
        pr_count: pr_bodies.len(),
        token_estimate: estimate_tokens(&prompt),
    });

    let result = HeadlessRunner::run(
        &AgentKind::Claude,
        &workdir,
        &prompt,
        model.as_deref(),
        false,
    )
    .and_then(|output| {
        let findings = review_memory::parse_findings_markdown(&output);
        let mut appended = 0;
        for (category, finding) in &findings {
            if review_memory::append_finding(&memory_path, memory_scope, category, finding)? {
                appended += 1;
            }
        }
        Ok(BootstrapOutcome {
            pr_count: pr_bodies.len(),
            appended,
        })
    });
    let _ = tx.send(BootstrapProgress::Done(result));
}

/// Findings currently in the review-memory doc at `path`, or `0` when it's
/// missing or unreadable. A local file read, cheap enough to do synchronously
/// when the compact overlay opens and again on every `g` scope toggle.
pub(super) fn count_findings_at(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|contents| review_memory::count_findings(&contents))
        .unwrap_or(0)
}

/// Background body of the review-memory compact pass ("prevent review-memory
/// rot"): read the doc, make **one** headless agent pass to merge
/// near-duplicate findings and prune stale ones, and report the proposed
/// replacement for the user to review — nothing is written here. Runs off the
/// UI thread; progress and the final result are reported over `tx`.
pub(super) fn run_review_memory_compact(
    workdir: PathBuf,
    memory_path: PathBuf,
    model: Option<String>,
    // The resolved `review_memory.compact` template (built-in or override),
    // rendered here once the doc is read on this worker thread.
    template: String,
    tx: std::sync::mpsc::Sender<CompactProgress>,
) {
    let contents = match std::fs::read_to_string(&memory_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = tx.send(CompactProgress::Done(Ok(None)));
            return;
        }
        Err(e) => {
            let _ = tx.send(CompactProgress::Done(Err(e.into())));
            return;
        }
    };

    let original_findings = review_memory::count_findings(&contents);
    if original_findings == 0 {
        let _ = tx.send(CompactProgress::Done(Ok(None)));
        return;
    }

    let prompt = crate::prompts::render_template(
        &template,
        &crate::prompts::PromptContext::new().with("doc_contents", contents.clone()),
    );
    let _ = tx.send(CompactProgress::Compacting {
        token_estimate: estimate_tokens(&prompt),
    });

    let result = HeadlessRunner::run(
        &AgentKind::Claude,
        &workdir,
        &prompt,
        model.as_deref(),
        false,
    )
    .map(|output| {
        let proposed_content = output.trim().to_string();
        let proposed_findings = review_memory::count_findings(&proposed_content);
        Some(CompactOutcome {
            original_findings,
            proposed_findings,
            proposed_content,
            original_content: contents,
        })
    });
    let _ = tx.send(CompactProgress::Done(result));
}

impl App {
    /// Open the "add to memory" dialog for the selected comment, seeded from
    /// [`PrComment::memory_finding_seed`] and defaulting to the `General`
    /// category. Editable before it's appended. No-op if a fix/reply/memory
    /// dialog is already open or nothing is selected.
    pub fn pr_review_open_memory_add(&mut self) {
        let seed = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.memory_add.is_none() =>
            {
                state
                    .selected_comment()
                    .filter(|c| c.is_actionable())
                    .map(|c| (c.id, c.memory_finding_seed()))
            }
            _ => return,
        };
        let Some((comment_id, seed)) = seed else {
            self.message = Some("No comment selected".into());
            return;
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = Some(MemoryAddState {
                comment_id,
                category: 0,
                scope: MemoryScope::Project,
                editor: TextEditor::new(seed),
                editing: false,
            });
        }
    }

    /// Enter edit mode so keystrokes flow to the finding editor.
    pub fn pr_review_memory_add_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the text is kept).
    pub fn pr_review_memory_add_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.editing = false;
        }
    }

    /// Forward a key to the open finding editor (only meaningful in edit mode).
    pub fn pr_review_memory_add_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
            && memory_add.editing
        {
            memory_add.editor.handle_key(key);
        }
    }

    /// Cycle the category (confirm view only) through [`MEMORY_CATEGORIES`].
    pub fn pr_review_cycle_memory_category(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.category = (memory_add.category + 1) % MEMORY_CATEGORIES.len();
        }
    }

    /// Toggle which doc the finding is appended to (confirm view only):
    /// this repo's committed doc, or the user's cross-project one.
    pub fn pr_review_toggle_memory_scope(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.scope = memory_add.scope.toggled();
        }
    }

    /// Close the "add to memory" dialog without appending.
    pub fn pr_review_cancel_memory_add(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = None;
        }
    }

    /// Memory-add dialog status for the key handler: `None` when closed, else
    /// whether it is currently in edit mode.
    pub fn pr_review_memory_add_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.memory_add.as_ref().map(|m| m.editing),
            _ => None,
        }
    }

    /// Append the (possibly edited) finding to the review-memory doc and close
    /// the dialog. Whitespace/newlines in the finding text are collapsed to a
    /// single line first, since the doc stores each finding as one bullet.
    /// Dedup-aware and append-only (`review_memory::append_finding`) — never
    /// touches existing prose. Zero agent tokens; a local file write only.
    pub fn pr_review_append_memory(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.memory_add.as_ref().map(|memory_add| {
                (
                    state.workdir.clone(),
                    MEMORY_CATEGORIES[memory_add.category],
                    memory_add.scope,
                    memory_add.editor.text(),
                )
            }),
            _ => return Ok(()),
        };
        let Some((workdir, category, scope, finding)) = prep else {
            return Ok(());
        };

        let finding = finding.split_whitespace().collect::<Vec<_>>().join(" ");
        if finding.is_empty() {
            self.message = Some("Finding is empty — type something or esc to cancel".into());
            return Ok(());
        }

        let repo = self.repo_for_project_path(&workdir);
        let paths = self.review_memory_paths(&repo);
        let appended =
            review_memory::append_finding(paths.for_scope(scope), scope, category, &finding)?;

        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = None;
        }
        let scope_label = scope.label();
        let toast = if appended {
            format!("Added to {scope_label} memory · {category}")
        } else {
            format!("Already in {scope_label} memory · skipped")
        };
        self.push_toast_success(toast);
        Ok(())
    }

    /// Open the lookback-bootstrap depth picker (`b` in the PR picker): an
    /// overlay on the picker, not a separate mode, mirroring how the fix
    /// harness picker overlays the review pane.
    pub fn open_review_memory_bootstrap_pick(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.bootstrap_pick = Some(BootstrapPickState {
                selected: BootstrapDepth::ALL
                    .iter()
                    .position(|d| *d == BootstrapDepth::default())
                    .unwrap_or(0),
                scope: MemoryScope::Project,
            });
        }
    }

    /// Toggle which doc the bootstrap's distilled findings are appended to:
    /// this repo's committed doc, or the user's cross-project one.
    pub fn review_memory_bootstrap_toggle_scope(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && let Some(pick) = &mut state.bootstrap_pick
        {
            pick.scope = pick.scope.toggled();
        }
    }

    /// Whether the bootstrap depth picker is currently open over the PR picker.
    pub fn review_memory_bootstrap_picking(&self) -> bool {
        matches!(&self.mode, AppMode::PrPicker(state) if state.bootstrap_pick.is_some())
    }

    /// Move the depth-picker highlight (`+1`/`-1`, wrapping).
    pub fn review_memory_bootstrap_pick_move(&mut self, delta: isize) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && let Some(pick) = &mut state.bootstrap_pick
        {
            let len = BootstrapDepth::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Close the depth picker without running anything, staying on the PR
    /// picker.
    pub fn review_memory_bootstrap_pick_cancel(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.bootstrap_pick = None;
        }
    }

    /// Confirm the chosen depth: resolve the recent closed/merged PRs
    /// synchronously (one cheap `gh` call), then hand the heavy work — the
    /// per-PR comment fetch loop and the one distill pass — to a background
    /// thread and switch to the full-screen running view.
    pub fn review_memory_bootstrap_pick_confirm(&mut self) {
        let (workdir, depth, scope, mut origin) = match &self.mode {
            AppMode::PrPicker(state) => {
                let Some(pick) = &state.bootstrap_pick else {
                    return;
                };
                let depth = BootstrapDepth::ALL[pick.selected];
                (state.workdir.clone(), depth, pick.scope, state.clone())
            }
            _ => return,
        };
        origin.bootstrap_pick = None;

        let entries = match GhCli::list_recent_closed_prs(&workdir, depth.limit()) {
            Ok(entries) => entries,
            Err(e) => {
                self.mode = AppMode::PrPicker(origin);
                self.show_error(e);
                return;
            }
        };
        if entries.is_empty() {
            self.mode = AppMode::PrPicker(origin);
            self.message = Some("No merged/closed PRs found to learn from".into());
            return;
        }

        let repo = self.repo_for_project_path(&workdir);
        let memory_path = self
            .review_memory_paths(&repo)
            .for_scope(scope)
            .to_path_buf();

        self.log_info(
            "pr_review",
            format!(
                "bootstrapping {} review memory from {} PRs (depth: {})",
                scope.label(),
                entries.len(),
                depth.label()
            ),
        );

        let model = self.config.review_model_for(ReviewAction::ReviewMemory);
        let (template, _) = self.resolve_headless_template(
            crate::prompts::PromptId::ReviewMemoryBootstrap,
            &AgentKind::Claude,
            &repo,
            &workdir,
        );
        let preview = format!(
            "{template}\n\n[the gathered PR comments/reviews are spliced into {{{{pr_history}}}} when the call runs]"
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::ReviewMemoryBootstrap,
            &AgentKind::Claude,
            &preview,
        ) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_memory_bootstrap_bg = Some(rx);
        let thread_workdir = workdir.clone();
        std::thread::spawn(move || {
            run_review_memory_bootstrap(
                thread_workdir,
                memory_path,
                scope,
                entries,
                model,
                template,
                tx,
            );
        });

        self.mode = AppMode::ReviewMemoryBootstrapRunning(BootstrapRunState {
            origin,
            depth,
            scope,
            stage: BootstrapStage::FetchingComments,
        });
    }

    /// Poll the background bootstrap. Progress messages update the running
    /// screen's stage; `Done` always surfaces a toast (success) or error (the
    /// run has a real side effect — tokens spent, findings written — even if
    /// the user already navigated away), and restores the PR picker only if
    /// the running screen is still showing. An error is also written onto the
    /// restored picker's own inline `error` field so it's visible immediately
    /// on return, not just logged. Returns `true` when a redraw is warranted.
    pub fn poll_review_memory_bootstrap_bg(&mut self) -> bool {
        let Some(rx) = self.review_memory_bootstrap_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(BootstrapProgress::Distilling {
                    pr_count,
                    token_estimate,
                }) => {
                    if let AppMode::ReviewMemoryBootstrapRunning(state) = &mut self.mode {
                        state.stage = BootstrapStage::Distilling {
                            pr_count,
                            token_estimate,
                        };
                    }
                    changed = true;
                }
                Ok(BootstrapProgress::Done(result)) => {
                    self.review_memory_bootstrap_bg = None;
                    // Capture the origin before any mode-mutating side effect
                    // below: `show_error` unconditionally resets `self.mode` to
                    // `Normal` for any non-Normal/Help/Viewing mode, which would
                    // otherwise clobber the running screen's stashed picker
                    // before we get a chance to restore it.
                    let (mut origin, scope) = match &self.mode {
                        AppMode::ReviewMemoryBootstrapRunning(state) => {
                            (Some(state.origin.clone()), state.scope)
                        }
                        _ => (None, MemoryScope::default()),
                    };
                    match result {
                        Ok(outcome) => {
                            self.push_toast_success(format!(
                                "Bootstrapped {} review memory from {} PR{} · {} new finding{}",
                                scope.label(),
                                outcome.pr_count,
                                if outcome.pr_count == 1 { "" } else { "s" },
                                outcome.appended,
                                if outcome.appended == 1 { "" } else { "s" },
                            ));
                        }
                        Err(e) => {
                            // `show_error` only logs and surfaces via the
                            // dashboard's status bar, which the PR picker's
                            // full-screen render doesn't draw — also set the
                            // picker's own inline `error` (the same field
                            // `pr_picker_choose` uses) so the failure is
                            // actually visible on return, not just logged.
                            let detail = e.to_string();
                            if let Some(origin) = &mut origin {
                                origin.error = Some(detail.clone());
                            }
                            self.show_error(e);
                        }
                    }
                    if let Some(origin) = origin {
                        self.mode = AppMode::PrPicker(origin);
                    }
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.review_memory_bootstrap_bg = None;
                    if let AppMode::ReviewMemoryBootstrapRunning(state) = &self.mode {
                        self.mode = AppMode::PrPicker(state.origin.clone());
                        self.message = Some("Bootstrap failed unexpectedly".to_string());
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the PR picker. The
    /// background thread isn't aborted — if it finishes later,
    /// [`App::poll_review_memory_bootstrap_bg`] still surfaces the result.
    pub fn cancel_review_memory_bootstrap(&mut self) {
        if let AppMode::ReviewMemoryBootstrapRunning(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }

    /// Open the review-memory compact confirm overlay (`c` in the PR picker):
    /// a synchronous local file read to show how many findings are currently
    /// in the doc before spending an agent pass on them (Epic E "prevent
    /// review-memory rot"). A no-op with a message only when *both* docs are
    /// missing or empty — there's nothing to compact anywhere. When just one
    /// has findings the overlay opens on that one, so an empty project doc
    /// doesn't block reaching a grown global one.
    pub fn open_review_memory_compact_confirm(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrPicker(state) => state.workdir.clone(),
            _ => return,
        };
        let repo = self.repo_for_project_path(&workdir);
        let paths = self.review_memory_paths(&repo);
        let project_findings = count_findings_at(paths.for_scope(MemoryScope::Project));
        let global_findings = count_findings_at(paths.for_scope(MemoryScope::Global));
        let (scope, existing_findings) = if project_findings > 0 {
            (MemoryScope::Project, project_findings)
        } else if global_findings > 0 {
            (MemoryScope::Global, global_findings)
        } else {
            self.message = Some("Review memory is empty — nothing to compact".into());
            return;
        };
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.compact_confirm = Some(CompactConfirmState {
                existing_findings,
                scope,
            });
        }
    }

    /// Toggle which doc the compact pass rewrites: this repo's committed doc,
    /// or the user's cross-project one. Re-reads the finding count for the
    /// newly selected doc so the overlay's "N findings" never describes the
    /// doc the user just toggled away from.
    pub fn review_memory_compact_toggle_scope(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrPicker(state) if state.compact_confirm.is_some() => state.workdir.clone(),
            _ => return,
        };
        let repo = self.repo_for_project_path(&workdir);
        let paths = self.review_memory_paths(&repo);
        if let AppMode::PrPicker(state) = &mut self.mode
            && let Some(confirm) = &mut state.compact_confirm
        {
            confirm.scope = confirm.scope.toggled();
            confirm.existing_findings = count_findings_at(paths.for_scope(confirm.scope));
        }
    }

    /// Whether the compact confirm overlay is currently open over the picker.
    pub fn review_memory_compact_confirming(&self) -> bool {
        matches!(&self.mode, AppMode::PrPicker(state) if state.compact_confirm.is_some())
    }

    /// Close the overlay without running anything, staying on the PR picker.
    pub fn review_memory_compact_confirm_cancel(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.compact_confirm = None;
        }
    }

    /// Confirm the overlay: hand the doc read + one agent pass to a
    /// background thread and switch to the full-screen running view. Refuses
    /// (with a message, staying on the overlay) when the selected doc is
    /// empty — reachable by toggling `g` onto a doc that has no findings, and
    /// not worth an agent pass.
    pub fn review_memory_compact_confirm_run(&mut self) {
        let (workdir, scope, empty, mut origin) = match &self.mode {
            AppMode::PrPicker(state) => match &state.compact_confirm {
                Some(confirm) => (
                    state.workdir.clone(),
                    confirm.scope,
                    confirm.existing_findings == 0,
                    state.clone(),
                ),
                None => return,
            },
            _ => return,
        };
        if empty {
            self.message = Some(format!(
                "The {} review memory doc is empty — nothing to compact",
                scope.label()
            ));
            return;
        }
        origin.compact_confirm = None;

        let repo = self.repo_for_project_path(&workdir);
        let memory_path = self
            .review_memory_paths(&repo)
            .for_scope(scope)
            .to_path_buf();

        self.log_info(
            "pr_review",
            format!("compacting {} review memory doc", scope.label()),
        );

        let model = self.config.review_model_for(ReviewAction::ReviewMemory);
        let (template, _) = self.resolve_headless_template(
            crate::prompts::PromptId::ReviewMemoryCompact,
            &AgentKind::Claude,
            &repo,
            &workdir,
        );
        let preview = format!(
            "{template}\n\n[the current review-memory doc is spliced into {{{{doc_contents}}}} when the call runs]"
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::ReviewMemoryCompact,
            &AgentKind::Claude,
            &preview,
        ) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_memory_compact_bg = Some(rx);
        let thread_workdir = workdir.clone();
        let thread_memory_path = memory_path.clone();
        std::thread::spawn(move || {
            run_review_memory_compact(thread_workdir, thread_memory_path, model, template, tx);
        });

        let run_state = CompactRunState {
            origin,
            path: memory_path,
            scope,
            stage: CompactStage::ReadingDoc,
        };
        self.review_memory_compact_pending = Some(run_state.clone());
        self.mode = AppMode::ReviewMemoryCompactRunning(run_state);
    }

    /// Poll the background compact pass. `Compacting` updates the running
    /// screen's token estimate; `Done` transitions to the full-screen review
    /// dialog on a successful rewrite (nothing is written yet), surfaces a
    /// message and returns to the picker when there was nothing to compact,
    /// or restores the picker with an inline error on failure — same
    /// restore-before-`show_error` ordering as
    /// [`App::poll_review_memory_bootstrap_bg`], for the same reason.
    pub fn poll_review_memory_compact_bg(&mut self) -> bool {
        let Some(rx) = self.review_memory_compact_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(CompactProgress::Compacting { token_estimate }) => {
                    if let AppMode::ReviewMemoryCompactRunning(state) = &mut self.mode {
                        state.stage = CompactStage::Compacting { token_estimate };
                    }
                    changed = true;
                }
                Ok(CompactProgress::Done(result)) => {
                    self.review_memory_compact_bg = None;
                    let Some(pending) = self.review_memory_compact_pending.take() else {
                        // Invariant: always set alongside `review_memory_compact_bg`
                        // in `review_memory_compact_confirm_run`. If it's ever
                        // missing there's nowhere safe to land the proposal.
                        changed = true;
                        break;
                    };
                    // Only auto-open the review dialog (or bounce a `None`/error
                    // back to the picker) if the user is still on the running
                    // screen. If they cancelled (`esc`) to the picker — or
                    // navigated anywhere else — nothing was written (unlike the
                    // bootstrap, which writes as it goes), so there's nowhere
                    // live to land a full-screen editable proposal without
                    // yanking the user out of whatever they're doing now; just
                    // surface that it finished.
                    let still_watching =
                        matches!(&self.mode, AppMode::ReviewMemoryCompactRunning(_));
                    match result {
                        Ok(Some(outcome)) => {
                            if still_watching {
                                self.mode =
                                    AppMode::ReviewMemoryCompactReview(CompactReviewState {
                                        origin: pending.origin,
                                        path: pending.path,
                                        scope: pending.scope,
                                        original_findings: outcome.original_findings,
                                        proposed_findings: outcome.proposed_findings,
                                        editor: TextEditor::new(outcome.proposed_content),
                                        original_content: outcome.original_content,
                                        overwrite_confirmed: false,
                                        editing: false,
                                        scroll: 0,
                                        sync_to_cursor: false,
                                        error: None,
                                    });
                            } else {
                                self.push_toast_info(
                                    "Review memory compact finished after you navigated away \
                                     — press c to re-run and review it"
                                        .to_string(),
                                );
                            }
                        }
                        Ok(None) => {
                            self.message = Some(format!(
                                "The {} review memory doc is empty — nothing to compact",
                                pending.scope.label()
                            ));
                            if still_watching {
                                self.mode = AppMode::PrPicker(pending.origin);
                            }
                        }
                        Err(e) => {
                            if still_watching {
                                let mut origin = pending.origin;
                                origin.error = Some(e.to_string());
                                self.show_error(e);
                                self.mode = AppMode::PrPicker(origin);
                            } else {
                                self.show_error(e);
                            }
                        }
                    }
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.review_memory_compact_bg = None;
                    self.review_memory_compact_pending = None;
                    if let AppMode::ReviewMemoryCompactRunning(state) = &self.mode {
                        self.mode = AppMode::PrPicker(state.origin.clone());
                        self.message = Some("Compact failed unexpectedly".to_string());
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the PR picker. The
    /// background thread isn't aborted — if it finishes later,
    /// [`App::poll_review_memory_compact_bg`] still notices (it doesn't
    /// auto-open the review dialog once the user isn't watching the running
    /// screen anymore, since nothing was written to land it against).
    pub fn cancel_review_memory_compact(&mut self) {
        if let AppMode::ReviewMemoryCompactRunning(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }

    /// Enter edit mode so keystrokes flow to the proposed-doc editor.
    pub fn pr_review_compact_review_edit(&mut self) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode {
            state.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the text is kept).
    pub fn pr_review_compact_review_stop_edit(&mut self) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode {
            state.editing = false;
        }
    }

    /// Whether the compact review dialog is in edit mode. `None` when the
    /// dialog isn't open.
    pub fn pr_review_compact_review_editing(&self) -> Option<bool> {
        match &self.mode {
            AppMode::ReviewMemoryCompactReview(state) => Some(state.editing),
            _ => None,
        }
    }

    /// Forward a key to the proposed-doc editor (only meaningful in edit mode).
    pub fn pr_review_compact_review_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode
            && state.editing
        {
            state.editor.handle_key(key);
            state.sync_to_cursor = true;
        }
    }

    /// Scroll the proposed-doc view (confirm view only).
    pub fn pr_review_compact_review_scroll(&mut self, delta: isize) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode {
            state.scroll = state.scroll.saturating_add_signed(delta);
            state.sync_to_cursor = false;
        }
    }

    /// Write the (possibly edited) proposed replacement to the review-memory
    /// doc and return to the PR picker. This is the one place the compact
    /// flow writes anything — the background pass only ever produces a
    /// proposal (see [`run_review_memory_compact`]). A write failure keeps
    /// the dialog open with the error shown inline, same as
    /// [`App::pr_review_post_ai_review`]'s recoverable-error handling.
    ///
    /// The proposal is a wholesale rewrite of a snapshot taken before the agent
    /// pass, so the doc is re-read here and compared against that snapshot
    /// first. Every other AMF flow only ever *appends* to these docs, and the
    /// cross-project doc is shared by every AMF session on the machine, so the
    /// common conflict is "another session appended findings while this dialog
    /// was open": those are replayed on top of the rewrite instead of being
    /// clobbered. A change an append can't explain (hand-edited prose, deleted
    /// findings) refuses the first write and reports it inline; confirming
    /// again overwrites deliberately.
    pub fn pr_review_compact_write(&mut self) -> Result<()> {
        let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode else {
            return Ok(());
        };
        let content = state.editor.text().to_string();

        let on_disk = match std::fs::read_to_string(&state.path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                state.error = Some(format!("Could not re-read the doc before writing: {e}"));
                return Ok(());
            }
        };
        let carried = match review_memory::doc_drift(&state.original_content, &on_disk) {
            review_memory::DocDrift::Unchanged => Vec::new(),
            review_memory::DocDrift::Appended(added) => added,
            review_memory::DocDrift::Diverged(added) => {
                if !state.overwrite_confirmed {
                    state.overwrite_confirmed = true;
                    state.error = Some(
                        "This doc changed on disk since the compact pass read it, in ways \
                         AMF can't re-apply on top of the rewrite. Confirm again to \
                         overwrite it anyway, or esc to discard and re-run the compact pass."
                            .to_string(),
                    );
                    return Ok(());
                }
                added
            }
        };

        let (content, restored) = review_memory::append_findings_to_doc(&content, &carried);
        match std::fs::write(&state.path, &content) {
            Ok(()) => {
                let (original, proposed) = (state.original_findings, state.proposed_findings);
                let scope = state.scope;
                let origin = state.origin.clone();
                self.mode = AppMode::PrPicker(origin);
                let carried_note = match restored {
                    0 => String::new(),
                    1 => " · kept 1 finding added elsewhere".to_string(),
                    n => format!(" · kept {n} findings added elsewhere"),
                };
                self.push_toast_success(format!(
                    "Compacted {} review memory · {original} \u{2192} {proposed} findings{carried_note}",
                    scope.label()
                ));
            }
            Err(e) => {
                state.error = Some(format!("Write failed: {e}"));
            }
        }
        Ok(())
    }

    /// Discard the proposed replacement without writing, returning to the PR
    /// picker.
    pub fn pr_review_compact_discard(&mut self) {
        if let AppMode::ReviewMemoryCompactReview(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }
}
