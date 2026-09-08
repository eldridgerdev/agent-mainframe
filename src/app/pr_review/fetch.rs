use super::{
    CommentKind, FixTarget, PrComment, PrReview, PrSortMode, SNIPPET_LEN, TRIAGE_SESSION_LABEL,
    TriageState,
};
use crate::app::{
    App, AppMode, PrNumberPromptState, PrPickerState, PrReviewLoadState, PrReviewState,
};
use crate::github::{
    GhCli, IssueComment, PrRef, PrResolution, Review, ReviewComment, ReviewThread,
};
use anyhow::Result;
use chrono::Local;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Fetch every comment source for a resolved PR and normalize them into one
/// [`PrReview`]. This is the single entry point the UI/state layer calls to
/// (re)load a review; it runs entirely in Rust and spends zero agent tokens.
///
/// Run this off the UI thread — it makes four `gh` calls.
pub fn fetch_and_normalize(workdir: &Path, pr: PrRef) -> Result<PrReview> {
    let review_comments = GhCli::pr_review_comments(workdir, pr.number)?;
    let reviews = GhCli::pr_reviews(workdir, pr.number)?;
    let issue_comments = GhCli::issue_comments(workdir, pr.number)?;
    let threads = GhCli::review_threads(workdir, &pr.owner, &pr.repo, pr.number)?;
    Ok(normalize(
        pr,
        review_comments,
        reviews,
        issue_comments,
        threads,
    ))
}

/// Merge the raw `gh` payloads into a single triage-ready [`PrReview`].
///
/// Inline comments come first (they're the core use case), then non-empty
/// review summaries, then conversation comments. Resolution state is attached
/// from the GraphQL thread map; empty-body review summaries (bare approvals)
/// are dropped since there's nothing to triage.
pub fn normalize(
    pr: PrRef,
    review_comments: Vec<ReviewComment>,
    reviews: Vec<Review>,
    issue_comments: Vec<IssueComment>,
    threads: Vec<ReviewThread>,
) -> PrReview {
    let thread_index = index_threads(&threads);
    let mut comments = Vec::new();

    for c in review_comments {
        let is_bot = c.user.is_bot();
        let (thread_id, is_resolved) = match thread_index.get(&c.id) {
            Some((id, resolved)) => (Some(id.clone()), *resolved),
            None => (None, false),
        };
        let snippet = make_snippet(&c.body, is_bot);
        // A file-level comment has no line by definition — that's not the same
        // thing as an outdated line comment, so don't badge it as one.
        let file_level = c.subject_type.as_deref() == Some("file");
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Inline,
            author: c.user.login,
            is_bot,
            path: c.path,
            line: c.line.or(c.original_line),
            side: c.side,
            outdated: c.line.is_none() && !file_level,
            file_level,
            diff_hunk: c.diff_hunk,
            body: c.body,
            snippet,
            in_reply_to: c.in_reply_to_id,
            thread_id,
            is_resolved,
            triage: TriageState::default(),
            local_note: None,
            batch_id: None,
            github_id: None,
            github_review_id: c.pull_request_review_id,
        });
    }

    for r in reviews {
        // A review with no body is just an approve/comment action — nothing to
        // triage, so skip it.
        if r.body.trim().is_empty() {
            continue;
        }
        let is_bot = r.user.is_bot();
        let snippet = make_snippet(&r.body, is_bot);
        comments.push(PrComment {
            id: r.id,
            kind: CommentKind::ReviewSummary { state: r.state },
            author: r.user.login,
            is_bot,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: r.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            batch_id: None,
            github_id: None,
            github_review_id: Some(r.id),
        });
    }

    for c in issue_comments {
        let is_bot = c.user.is_bot();
        let snippet = make_snippet(&c.body, is_bot);
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Conversation,
            author: c.user.login,
            is_bot,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: c.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            batch_id: None,
            github_id: None,
            github_review_id: None,
        });
    }

    PrReview {
        pr,
        comments,
        fetched_at: Local::now(),
    }
}

/// Build `comment_id -> (thread_node_id, is_resolved)` from the GraphQL threads.
pub(super) fn index_threads(threads: &[ReviewThread]) -> HashMap<u64, (String, bool)> {
    let mut map = HashMap::new();
    for t in threads {
        for &cid in &t.comment_ids {
            map.insert(cid, (t.id.clone(), t.is_resolved));
        }
    }
    map
}

/// Produce a one-line list snippet, stripping bot boilerplate first so the
/// snippet reflects the actual content, not scaffolding.
pub(super) fn make_snippet(body: &str, is_bot: bool) -> String {
    let cleaned = if is_bot {
        strip_bot_boilerplate(body)
    } else {
        body.to_string()
    };
    let first = cleaned
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    truncate_chars(first, SNIPPET_LEN)
}

/// Strip the heavy scaffolding bots (CodeRabbit, Copilot, …) wrap around their
/// actual point: `<details>` blocks, HTML comments, `<summary>` tags, markdown
/// image badges, fenced quoted-diff/suggestion blocks, and leading `> `
/// quoted-diff lines. Cheap and lossy-by-design — only the actionable prose
/// needs to survive for the agent prompt and snippet. The comment's own
/// `diff_hunk` (plus the checked-out repo) already gives the agent this
/// context, so a bot re-quoting the diff inline is pure repetition.
pub fn strip_bot_boilerplate(body: &str) -> String {
    static DETAILS: OnceLock<Regex> = OnceLock::new();
    static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static QUOTED_DIFF_FENCE: OnceLock<Regex> = OnceLock::new();
    static QUOTED_LINES: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();

    let details = DETAILS.get_or_init(|| Regex::new(r"(?is)<details>.*?</details>").unwrap());
    let html_comment = HTML_COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    let summary = SUMMARY.get_or_init(|| Regex::new(r"(?is)</?summary>").unwrap());
    let image = IMAGE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    // Fenced ```diff / ```suggestion blocks: bots paste the same hunk back as
    // a code fence, which repeats context the agent already gets for free
    // from `diff_hunk`.
    let quoted_diff_fence = QUOTED_DIFF_FENCE
        .get_or_init(|| Regex::new(r"(?ims)^```(?:diff|suggestion)\s*\n.*?\n```\s*$").unwrap());
    // Leading `> ` blockquote lines (bots sometimes quote the diff as a
    // blockquote instead of a fence).
    let quoted_lines = QUOTED_LINES.get_or_init(|| Regex::new(r"(?m)^>.*$\n?").unwrap());
    let blanks = BLANKS.get_or_init(|| Regex::new(r"\n{3,}").unwrap());

    let s = details.replace_all(body, "");
    let s = html_comment.replace_all(&s, "");
    let s = summary.replace_all(&s, "");
    let s = image.replace_all(&s, "");
    let s = quoted_diff_fence.replace_all(&s, "");
    let s = quoted_lines.replace_all(&s, "");
    let s = blanks.replace_all(&s, "\n\n");
    s.trim().to_string()
}

/// Truncate to `max` characters (not bytes), appending an ellipsis when cut.
pub(super) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

impl App {
    /// Open PR Triage for the selected feature's branch.
    ///
    /// Runs the `gh` preconditions and resolves the PR synchronously (cheap),
    /// then kicks the comment fetch onto a background thread. All of this
    /// spends zero agent tokens.
    pub fn open_pr_review(&mut self) {
        let Some((_project, feature)) = self.selected_feature() else {
            self.message = Some("Select a feature to review its PR".to_string());
            return;
        };
        let workdir = feature.workdir.clone();
        self.open_pr_review_for_workdir(workdir);
    }

    /// Open PR Triage for the feature behind the current `Viewing` session —
    /// the leader-key entry point (`leader+G`), peer to the dashboard's `G`.
    /// Lets the user jump straight into triage without first exiting to the
    /// dashboard and re-entering.
    pub fn open_pr_review_from_view(&mut self) {
        let AppMode::Viewing(view) = &self.mode else {
            return;
        };
        let Some(workdir) = self.feature_for_view(view).map(|f| f.workdir.clone()) else {
            self.message = Some("No active feature to review".to_string());
            return;
        };
        self.open_pr_review_for_workdir(workdir);
    }

    pub(super) fn open_pr_review_for_workdir(&mut self, workdir: PathBuf) {
        if let Err(e) = GhCli::check_available() {
            self.show_error(e);
            return;
        }
        if let Err(e) = GhCli::check_auth() {
            self.show_error(e);
            return;
        }

        match GhCli::resolve_pr(&workdir) {
            Ok(PrResolution::Found(pr)) => self.enter_pr_review(workdir, pr),
            // No PR for this branch: offer a list of the repo's PRs to pick from
            // (the picker falls through to the number prompt on its own if the
            // list can't be fetched).
            Ok(PrResolution::NoPrForBranch) => self.open_pr_picker(workdir, None),
            Err(e) => self.show_error(e),
        }
    }

    /// Open the PR Triage pane for a resolved PR, preferring the SQLite cache.
    ///
    /// A cache hit (same `PR# + head SHA`) skips the four `gh` calls entirely and
    /// shows the stored comments instantly; a miss falls back to the background
    /// fetch. Either path spends zero agent tokens. Manual refresh
    /// ([`refresh_pr_review`](Self::refresh_pr_review)) bypasses the cache.
    pub(crate) fn enter_pr_review(&mut self, workdir: PathBuf, pr: PrRef) {
        if let Some(mut review) = self.load_cached_pr_review(&pr) {
            self.log_info(
                "pr_review",
                format!("cache hit for PR #{} @ {}", pr.number, pr.head_sha),
            );
            self.apply_persisted_triage(&mut review);
            let investigations = self.pr_review_load_investigations(&workdir, review.pr.number);
            let usage_baselines = self.pr_review_initial_usage_baselines(&workdir);
            let ai_review = self.ai_review_triage_snapshot(&review.pr);
            let checked_out_branch =
                crate::worktree::WorktreeManager::current_branch(&workdir).unwrap_or(None);
            self.mode = AppMode::PrReview(PrReviewState {
                workdir,
                review,
                selected: 0,
                detail_scroll: 0,
                detail_content_lines: 0,
                hide_resolved: false,
                sort_mode: PrSortMode::default(),
                fix_target: FixTarget::default(),
                fix_target_picked: false,
                usage_baselines,
                review_harness: None,
                dedicated_session_label: TRIAGE_SESSION_LABEL.to_string(),
                harness_pick: None,
                new_feature_setup: None,
                integrate: None,
                fix_confirm: None,
                fix_vim_enabled: false,
                mark_pick: None,
                reply_kind_pick: None,
                reply: None,
                memory_add: None,
                marked: std::collections::HashSet::new(),
                pending_batch: false,
                checked_out_branch,
                pending_ai_review_findings: ai_review.pending_findings,
                ai_review_last_run: ai_review.last_run,
                investigations,
                investigation_harness_pick: None,
                investigation_action_pick: None,
                investigation_follow_up: None,
                pending_follow_up: None,
                investigation_context: Default::default(),
            });
            // A companion triage feature created on an earlier visit is reused
            // for every fix in this PR — adopt it now so `f` doesn't re-ask.
            self.adopt_existing_triage_feature();
            return;
        }
        self.start_pr_review_fetch(workdir, pr);
    }

    /// Drop in-memory state that belongs to a known predecessor PR on the same
    /// feature.
    /// SQLite cache and triage rows are intentionally untouched: they remain
    /// keyed by PR number and are still available when the user explicitly
    /// chooses a closed PR from the picker.
    ///
    /// This is called when dashboard badge sync observes a PR-number transition.
    /// Naming the predecessor explicitly keeps an older PR chosen from the picker
    /// from invalidating live work that belongs to the current successor. It
    /// prevents `leader+P`, a late AI review result, or an old comment-fetch
    /// result from silently restoring the closed predecessor after the branch
    /// has been reused.
    pub(crate) fn invalidate_pr_context_for_transition(
        &mut self,
        workdir: &Path,
        predecessor_pr_number: u32,
    ) -> bool {
        let mut changed = false;

        if self.pr_review_return.as_ref().is_some_and(|stash| {
            stash.state.workdir == workdir && stash.state.review.pr.number == predecessor_pr_number
        }) {
            self.pr_review_return = None;
            changed = true;
        }

        if self.ai_review_pending.as_ref().is_some_and(|pending| {
            pending.workdir == workdir && pending.pr.number == predecessor_pr_number
        }) {
            self.ai_review_pending = None;
            self.ai_review_bg = None;
            self.ai_review_progress = None;
            changed = true;
        }

        if self
            .ai_review_triage_refresh_pending
            .as_ref()
            .is_some_and(|pending| {
                pending.workdir == workdir && pending.pr.number == predecessor_pr_number
            })
        {
            self.ai_review_triage_refresh_pending = None;
            self.ai_review_triage_refresh_bg = None;
            changed = true;
        }

        let stale_loading = matches!(
            &self.mode,
            AppMode::PrReviewLoading(state)
                if state.workdir == workdir && state.pr.number == predecessor_pr_number
        );
        let stale_ai_run = matches!(
            &self.mode,
            AppMode::AiReviewRunning(state)
                if state.origin.workdir == workdir
                    && state.origin.pr.number == predecessor_pr_number
        );
        if stale_loading {
            self.pr_review_bg = None;
            self.mode = AppMode::Normal;
            changed = true;
        } else if stale_ai_run {
            self.ai_review_bg = None;
            self.ai_review_pending = None;
            self.ai_review_progress = None;
            self.mode = AppMode::Normal;
            changed = true;
        }

        changed
    }

    /// Look up a cached, normalized review for this PR's head SHA. Returns `None`
    /// on a miss, when there's no DB, or when the cache read fails (non-fatal —
    /// the caller just re-fetches).
    pub(super) fn load_cached_pr_review(&self, pr: &PrRef) -> Option<PrReview> {
        self.db
            .as_ref()?
            .load_pr_review_cache(pr.number, &pr.head_sha)
            .ok()
            .flatten()
    }

    /// Persist a freshly-fetched review under its `PR# + head SHA` key so the
    /// next open is a cache hit. A write failure is non-fatal (logged, not shown).
    pub(crate) fn cache_pr_review(&mut self, review: &PrReview) {
        let result = match self.db.as_ref() {
            Some(db) => db.save_pr_review_cache(review),
            None => return,
        };
        if let Err(e) = result {
            self.log_warn("pr_review", format!("cache write failed: {e}"));
        }
    }

    /// Overlay the persisted local triage (`Fixing`/`Done`/skip notes) onto a
    /// freshly-loaded review. The `pr_comment_triage` table — keyed by
    /// `PR# + comment id` (not the head SHA, so marks survive a push that moves
    /// the PR's head) — is authoritative for local triage, so it wins over
    /// whatever the cache blob happened to serialize. A read failure (or no DB)
    /// is non-fatal: comments just stay [`TriageState::Untriaged`].
    pub(crate) fn apply_persisted_triage(&mut self, review: &mut PrReview) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let triage = match db.load_pr_comment_triage(review.pr.number) {
            Ok(map) => map,
            Err(e) => {
                self.log_warn("pr_review", format!("triage load failed: {e}"));
                return;
            }
        };
        for comment in &mut review.comments {
            if let Some(row) = triage.get(&comment.id) {
                comment.triage = row.state;
                comment.local_note = row.note.clone();
                comment.batch_id = row.batch_id.clone();
            }
        }
    }

    /// Open the PR picker: a selectable list of the repo's PRs. `seed_number`
    /// pre-highlights that PR when present (e.g. the branch's auto-detected one,
    /// or the PR already open in the pane). Lists open PRs by default. If `gh pr
    /// list` fails outright, falls back to the manual number prompt so the user
    /// is never stuck. Zero agent tokens.
    pub fn open_pr_picker(&mut self, workdir: PathBuf, seed_number: Option<u32>) {
        match GhCli::list_prs(&workdir, false) {
            Ok(entries) => {
                let selected = seed_number
                    .and_then(|n| entries.iter().position(|e| e.number == n))
                    .unwrap_or(0);
                let current_user = self.resolve_gh_current_user(&workdir);
                self.mode = AppMode::PrPicker(PrPickerState {
                    workdir,
                    entries,
                    selected,
                    include_closed: false,
                    error: None,
                    bootstrap_pick: None,
                    compact_confirm: None,
                    current_user,
                });
            }
            Err(e) => {
                self.log_warn("pr_review", format!("pr list failed: {e}"));
                self.prompt_pr_number(workdir, Some(e.to_string()));
            }
        }
    }

    /// Resolve the authenticated `gh` user's login, memoized in
    /// [`App::gh_current_user`] for the session so the PR picker doesn't
    /// repeat the `gh api user` call on every open/refresh. A failed
    /// resolution (e.g. `gh` unauthenticated) is cached too, rather than
    /// retried on every call.
    pub(crate) fn resolve_gh_current_user(&mut self, workdir: &Path) -> Option<String> {
        if let Some(cached) = &self.gh_current_user {
            return cached.clone();
        }
        let resolved = match GhCli::current_user(workdir) {
            Ok(login) => Some(login),
            Err(e) => {
                self.log_warn("pr_review", format!("could not resolve gh user: {e}"));
                None
            }
        };
        self.gh_current_user = Some(resolved.clone());
        resolved
    }

    /// Open the PR picker from PR Triage (the `g` key), seeded on the
    /// PR currently being reviewed so it starts highlighted.
    pub fn open_pr_picker_from_pane(&mut self) {
        let (workdir, current) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), Some(state.review.pr.number)),
            AppMode::PrReviewLoading(state) => (state.workdir.clone(), Some(state.pr.number)),
            _ => return,
        };
        self.open_pr_picker(workdir, current);
    }

    /// Move the picker highlight down one row (clamped).
    pub fn pr_picker_select_next(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && !state.entries.is_empty()
        {
            state.selected = (state.selected + 1).min(state.entries.len() - 1);
        }
    }

    /// Move the picker highlight up one row (clamped).
    pub fn pr_picker_select_prev(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.selected = state.selected.saturating_sub(1);
        }
    }

    /// Toggle whether the picker list includes closed/merged PRs, re-fetching
    /// with the new filter. Keeps the highlight on the same PR number when it
    /// survives the toggle.
    pub fn pr_picker_toggle_closed(&mut self) {
        let (workdir, include_closed, current) = match &self.mode {
            AppMode::PrPicker(state) => (
                state.workdir.clone(),
                !state.include_closed,
                state.entries.get(state.selected).map(|e| e.number),
            ),
            _ => return,
        };
        match GhCli::list_prs(&workdir, include_closed) {
            Ok(entries) => {
                let selected = current
                    .and_then(|n| entries.iter().position(|e| e.number == n))
                    .unwrap_or(0);
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.entries = entries;
                    state.selected = selected;
                    state.include_closed = include_closed;
                    state.error = None;
                }
            }
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Resolve the highlighted PR (by number) and open it for review. On a
    /// resolve failure the picker stays open with an inline error.
    pub fn pr_picker_choose(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrPicker(state) => match state.entries.get(state.selected) {
                Some(entry) => (state.workdir.clone(), entry.number),
                None => return,
            },
            _ => return,
        };
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.enter_pr_review(workdir, pr),
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Switch from the picker to the manual PR-number prompt (the `#` key), so
    /// "pick a PR" and "type a number" live behind one entry point.
    pub fn pr_picker_to_number_prompt(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrPicker(state) => state.workdir.clone(),
            _ => return,
        };
        self.prompt_pr_number(workdir, None);
    }

    /// Open the manual PR-number override prompt. Used when the branch has no
    /// auto-detectable open PR; `error` seeds an inline message after a failed
    /// resolve so the user can correct the number and retry.
    pub(super) fn prompt_pr_number(&mut self, workdir: PathBuf, error: Option<String>) {
        self.mode = AppMode::PrNumberPrompt(PrNumberPromptState {
            workdir,
            input: String::new(),
            error,
        });
    }

    /// Append a digit to the PR-number prompt (non-digits are ignored).
    pub fn pr_number_prompt_push(&mut self, c: char) {
        if let AppMode::PrNumberPrompt(state) = &mut self.mode
            && c.is_ascii_digit()
        {
            state.input.push(c);
        }
    }

    /// Delete the last digit from the PR-number prompt.
    pub fn pr_number_prompt_backspace(&mut self) {
        if let AppMode::PrNumberPrompt(state) = &mut self.mode {
            state.input.pop();
        }
    }

    /// Resolve the typed PR number and, on success, start the comment fetch.
    /// On failure the prompt stays open with an inline error so the user can
    /// retry. Spends zero agent tokens (one `gh pr view <n>` call).
    pub fn submit_pr_number(&mut self) {
        let AppMode::PrNumberPrompt(state) = &self.mode else {
            return;
        };
        let workdir = state.workdir.clone();
        let Ok(number) = state.input.trim().parse::<u32>() else {
            self.prompt_pr_number(workdir, Some("Enter a PR number, e.g. 321".to_string()));
            return;
        };

        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.enter_pr_review(workdir, pr),
            Err(e) => self.prompt_pr_number(workdir, Some(e.to_string())),
        }
    }

    /// Re-fetch the currently-open PR, bypassing the cache. Re-resolves the PR
    /// first so a new head SHA (e.g. after pushing fixes) is picked up, then
    /// fetches fresh comments and overwrites the cache row. Zero agent tokens.
    pub fn refresh_pr_review(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.number),
            _ => return,
        };
        self.log_info("pr_review", format!("refreshing PR #{number}"));
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.start_pr_review_fetch(workdir, pr),
            Err(e) => self.show_error(e),
        }
    }

    /// Spawn the off-thread comment fetch and enter the loading mode.
    pub(super) fn start_pr_review_fetch(&mut self, workdir: PathBuf, pr: PrRef) {
        self.log_info(
            "pr_review",
            format!("fetching comments for PR #{}", pr.number),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        self.pr_review_bg = Some(rx);

        let usage_baselines = match &self.mode {
            AppMode::PrReview(state) if state.review.pr.number == pr.number => {
                state.usage_baselines.clone()
            }
            _ => self.pr_review_initial_usage_baselines(&workdir),
        };

        let thread_workdir = workdir.clone();
        let thread_pr = pr.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_and_normalize(&thread_workdir, thread_pr));
        });

        self.mode = AppMode::PrReviewLoading(PrReviewLoadState {
            workdir,
            pr,
            usage_baselines,
        });
    }

    /// Whether a PR comment fetch is in flight.
    pub fn pr_review_loading(&self) -> bool {
        matches!(self.mode, AppMode::PrReviewLoading(_))
    }

    /// Poll the background PR fetch. On completion, transition to the review
    /// pane (or report the error and return to the dashboard). Returns `true`
    /// when state changed and a redraw is warranted.
    pub fn poll_pr_review_bg(&mut self) -> bool {
        let Some(rx) = self.pr_review_bg.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.pr_review_bg = None;
                // If the user navigated away from the loading screen, drop it.
                let AppMode::PrReviewLoading(state) = &self.mode else {
                    return false;
                };
                let workdir = state.workdir.clone();
                let usage_baselines = state.usage_baselines.clone();
                match result {
                    Ok(mut review) => {
                        self.log_info(
                            "pr_review",
                            format!("loaded {} comments", review.comments.len()),
                        );
                        self.cache_pr_review(&review);
                        self.apply_persisted_triage(&mut review);
                        let investigations =
                            self.pr_review_load_investigations(&workdir, review.pr.number);
                        let ai_review = self.ai_review_triage_snapshot(&review.pr);
                        let checked_out_branch =
                            crate::worktree::WorktreeManager::current_branch(&workdir)
                                .unwrap_or(None);
                        self.mode = AppMode::PrReview(PrReviewState {
                            workdir,
                            review,
                            selected: 0,
                            detail_scroll: 0,
                            detail_content_lines: 0,
                            hide_resolved: false,
                            sort_mode: PrSortMode::default(),
                            fix_target: FixTarget::default(),
                            fix_target_picked: false,
                            usage_baselines,
                            review_harness: None,
                            dedicated_session_label: TRIAGE_SESSION_LABEL.to_string(),
                            harness_pick: None,
                            new_feature_setup: None,
                            integrate: None,
                            fix_confirm: None,
                            fix_vim_enabled: false,
                            mark_pick: None,
                            reply_kind_pick: None,
                            reply: None,
                            memory_add: None,
                            marked: std::collections::HashSet::new(),
                            pending_batch: false,
                            checked_out_branch,
                            pending_ai_review_findings: ai_review.pending_findings,
                            ai_review_last_run: ai_review.last_run,
                            investigations,
                            investigation_harness_pick: None,
                            investigation_action_pick: None,
                            investigation_follow_up: None,
                            pending_follow_up: None,
                            investigation_context: Default::default(),
                        });
                        self.adopt_existing_triage_feature();
                    }
                    Err(e) => {
                        self.mode = AppMode::Normal;
                        self.show_error(e);
                    }
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_bg = None;
                if matches!(self.mode, AppMode::PrReviewLoading(_)) {
                    self.mode = AppMode::Normal;
                    self.message = Some("PR fetch failed unexpectedly".to_string());
                    return true;
                }
                false
            }
        }
    }

    /// Re-fetch GitHub review-thread resolution state and apply it to the
    /// in-memory review (and cache). One GraphQL call, zero agent tokens. A
    /// failure is non-fatal — the existing markers just stay as they were.
    pub(super) fn refresh_thread_resolution(&mut self) {
        let (workdir, pr) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
            _ => return,
        };
        let threads = match GhCli::review_threads(&workdir, &pr.owner, &pr.repo, pr.number) {
            Ok(threads) => threads,
            Err(e) => {
                self.log_warn("pr_review", format!("thread refresh failed: {e}"));
                return;
            }
        };
        let index = index_threads(&threads);
        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                let github_id = c.github_id.unwrap_or(c.id);
                if let Some((tid, resolved)) = index.get(&github_id) {
                    c.thread_id = Some(tid.clone());
                    c.is_resolved = *resolved;
                }
            }
        }
        self.recache_current_review();
    }

    /// Re-persist the current in-memory review to the SQLite cache so a later
    /// cache-hit re-open reflects resolution changes made in the pane.
    pub(super) fn recache_current_review(&mut self) {
        let review = match &self.mode {
            AppMode::PrReview(state) => state.review.clone(),
            _ => return,
        };
        self.cache_pr_review(&review);
    }
}
