//! PR comment-review model and normalization (feature-specific).
//!
//! The generic `gh` access lives in [`crate::github`]; this module turns those
//! raw GitHub payloads into a single triage-ready [`PrReview`] and owns the
//! token-saving transforms (bot-boilerplate stripping, one-line snippets,
//! thread-resolution merge). See `docs/backlog/pr-comment-review-plan.md`.

// Wired into the App state / UI layer in the next step; until then the model
// and helpers are exercised only by unit tests.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use chrono::{DateTime, Local};
use regex::Regex;

use super::*;
use crate::github::{GhCli, IssueComment, PrRef, PrResolution, Review, ReviewComment, ReviewThread};

/// Snippet length (chars) shown in the comment list.
const SNIPPET_LEN: usize = 80;

/// What kind of GitHub comment this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentKind {
    /// Inline review comment anchored to a file/line.
    Inline,
    /// A review summary body (Approve / Request changes / Comment).
    ReviewSummary { state: String },
    /// A conversation comment on the PR timeline (no code anchor).
    Conversation,
}

/// Local triage decision, cached in SQLite later. GitHub thread resolution is
/// the source of truth for "done"; this is the local layer on top of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriageState {
    #[default]
    Untriaged,
    Fixing,
    Done,
    Skipped,
    Replied,
}

/// One normalized, display-ready comment.
#[derive(Debug, Clone)]
pub struct PrComment {
    pub id: u64,
    pub kind: CommentKind,
    pub author: String,
    pub is_bot: bool,
    pub path: Option<String>,
    /// Best-known line: the current diff line, falling back to the original.
    pub line: Option<u32>,
    /// True when the comment's anchor line no longer exists in the diff.
    pub outdated: bool,
    pub diff_hunk: Option<String>,
    /// Original comment body as returned by GitHub.
    pub body: String,
    /// One-line snippet for the list (boilerplate-stripped, truncated).
    pub snippet: String,
    pub in_reply_to: Option<u64>,
    /// GraphQL review-thread node id (inline comments that belong to a thread).
    pub thread_id: Option<String>,
    /// Resolution state from GitHub (source of truth for done/not-done).
    pub is_resolved: bool,
    pub triage: TriageState,
    pub local_note: Option<String>,
}

impl PrComment {
    /// Text to send to the agent: boilerplate-stripped for bots, verbatim for
    /// humans. Keeps token-heavy bot scaffolding out of prompts.
    pub fn agent_text(&self) -> String {
        if self.is_bot {
            strip_bot_boilerplate(&self.body)
        } else {
            self.body.clone()
        }
    }
}

/// A fully normalized PR review: the resolved PR plus every triageable comment.
#[derive(Debug, Clone)]
pub struct PrReview {
    pub pr: PrRef,
    pub comments: Vec<PrComment>,
    pub fetched_at: DateTime<Local>,
}

impl PrReview {
    /// Number of comments not yet resolved on GitHub.
    pub fn open_count(&self) -> usize {
        self.comments.iter().filter(|c| !c.is_resolved).count()
    }
}

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
    Ok(normalize(pr, review_comments, reviews, issue_comments, threads))
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
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Inline,
            author: c.user.login,
            is_bot,
            path: c.path,
            line: c.line.or(c.original_line),
            outdated: c.line.is_none(),
            diff_hunk: c.diff_hunk,
            body: c.body,
            snippet,
            in_reply_to: c.in_reply_to_id,
            thread_id,
            is_resolved,
            triage: TriageState::default(),
            local_note: None,
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
            outdated: false,
            diff_hunk: None,
            body: r.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
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
            outdated: false,
            diff_hunk: None,
            body: c.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
        });
    }

    PrReview {
        pr,
        comments,
        fetched_at: Local::now(),
    }
}

/// Build `comment_id -> (thread_node_id, is_resolved)` from the GraphQL threads.
fn index_threads(threads: &[ReviewThread]) -> HashMap<u64, (String, bool)> {
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
fn make_snippet(body: &str, is_bot: bool) -> String {
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
/// actual point: `<details>` blocks, HTML comments, `<summary>` tags, and
/// markdown image badges. Cheap and lossy-by-design — only the actionable prose
/// needs to survive for the agent prompt and snippet.
pub fn strip_bot_boilerplate(body: &str) -> String {
    static DETAILS: OnceLock<Regex> = OnceLock::new();
    static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();

    let details = DETAILS.get_or_init(|| Regex::new(r"(?is)<details>.*?</details>").unwrap());
    let html_comment = HTML_COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    let summary = SUMMARY.get_or_init(|| Regex::new(r"(?is)</?summary>").unwrap());
    let image = IMAGE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    let blanks = BLANKS.get_or_init(|| Regex::new(r"\n{3,}").unwrap());

    let s = details.replace_all(body, "");
    let s = html_comment.replace_all(&s, "");
    let s = summary.replace_all(&s, "");
    let s = image.replace_all(&s, "");
    let s = blanks.replace_all(&s, "\n\n");
    s.trim().to_string()
}

/// Truncate to `max` characters (not bytes), appending an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

impl App {
    /// Open the PR comment-review pane for the selected feature's branch.
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

        if let Err(e) = GhCli::check_available() {
            self.show_error(e);
            return;
        }
        if let Err(e) = GhCli::check_auth() {
            self.show_error(e);
            return;
        }

        match GhCli::resolve_pr(&workdir) {
            Ok(PrResolution::Found(pr)) => self.start_pr_review_fetch(workdir, pr),
            Ok(PrResolution::NoPrForBranch) => {
                self.message =
                    Some("No open PR for this branch (manual PR entry coming soon)".to_string());
            }
            Err(e) => self.show_error(e),
        }
    }

    /// Spawn the off-thread comment fetch and enter the loading mode.
    fn start_pr_review_fetch(&mut self, workdir: PathBuf, pr: PrRef) {
        self.log_info(
            "pr_review",
            format!("fetching comments for PR #{}", pr.number),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        self.pr_review_bg = Some(rx);

        let thread_workdir = workdir.clone();
        let thread_pr = pr.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_and_normalize(&thread_workdir, thread_pr));
        });

        self.mode = AppMode::PrReviewLoading(PrReviewLoadState { workdir, pr });
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
                match result {
                    Ok(review) => {
                        self.log_info(
                            "pr_review",
                            format!("loaded {} comments", review.comments.len()),
                        );
                        self.mode = AppMode::PrReview(PrReviewState {
                            workdir,
                            review,
                            selected: 0,
                            detail_scroll: 0,
                            hide_resolved: false,
                        });
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

    /// Close the review pane / cancel a pending load and return to the dashboard.
    pub fn close_pr_review(&mut self) {
        self.pr_review_bg = None;
        self.mode = AppMode::Normal;
    }

    pub fn pr_review_select_next(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            if let Some(next) = visible.iter().find(|&&i| i > state.selected) {
                state.selected = *next;
                state.detail_scroll = 0;
            }
        }
    }

    pub fn pr_review_select_prev(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            if let Some(prev) = visible.iter().rev().find(|&&i| i < state.selected) {
                state.selected = *prev;
                state.detail_scroll = 0;
            }
        }
    }

    /// Toggle hiding GitHub-resolved comments. When the current selection
    /// becomes hidden, snap to the nearest remaining visible comment.
    pub fn pr_review_toggle_resolved(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.hide_resolved = !state.hide_resolved;
            let visible = state.visible_indices();
            if visible.is_empty() {
                return;
            }
            if !visible.contains(&state.selected) {
                // Prefer the next visible comment after the old selection,
                // else the last visible one before it.
                state.selected = visible
                    .iter()
                    .find(|&&i| i >= state.selected)
                    .or_else(|| visible.last())
                    .copied()
                    .unwrap_or(0);
                state.detail_scroll = 0;
            }
        }
    }

    pub fn pr_review_scroll_detail_up(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.detail_scroll = state.detail_scroll.saturating_sub(amount);
        }
    }

    pub fn pr_review_scroll_detail_down(&mut self, amount: usize) {
        let max_scroll = self.pr_review_detail_line_count().saturating_sub(1);
        if let AppMode::PrReview(state) = &mut self.mode {
            state.detail_scroll = (state.detail_scroll + amount).min(max_scroll);
        }
    }

    /// Raw line count of the selected comment's detail content, used to clamp
    /// detail scrolling. Mirrors the line structure built in
    /// `ui::dialogs::pr_review::draw_comment_detail`.
    fn pr_review_detail_line_count(&self) -> usize {
        match &self.mode {
            AppMode::PrReview(state) => {
                state.selected_comment().map_or(0, PrComment::detail_line_count)
            }
            _ => 0,
        }
    }
}

impl PrComment {
    /// Raw line count of this comment's detail rendering. Must stay in sync with
    /// `ui::dialogs::pr_review::draw_comment_detail`: location header, author
    /// line, an optional blank + diff hunk, then a blank + body.
    pub fn detail_line_count(&self) -> usize {
        let mut n = 2; // location header + author line
        if let Some(hunk) = &self.diff_hunk {
            n += 1 + hunk.lines().count();
        }
        n += 1 + self.body.lines().count();
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GhUser;

    fn pr() -> PrRef {
        PrRef {
            number: 1,
            head_sha: "sha".into(),
            url: "https://github.com/o/r/pull/1".into(),
            owner: "o".into(),
            repo: "r".into(),
        }
    }

    fn user(login: &str, kind: &str) -> GhUser {
        GhUser {
            login: login.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn strips_details_comments_and_images() {
        let body = "Real point here.\n\n<details>\n<summary>Prompt for AI agents</summary>\n\
            lots of tokens\n</details>\n<!-- internal note -->\n![badge](http://x/y.png)";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Real point here.");
    }

    #[test]
    fn snippet_truncates_with_ellipsis() {
        let long = "x".repeat(200);
        let s = truncate_chars(&long, SNIPPET_LEN);
        assert_eq!(s.chars().count(), SNIPPET_LEN);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn snippet_skips_leading_blank_lines() {
        assert_eq!(make_snippet("\n\n  hello world  \n", false), "hello world");
    }

    #[test]
    fn normalize_attaches_resolution_and_outdated() {
        let comments = vec![
            ReviewComment {
                id: 11,
                path: Some("a.rs".into()),
                line: None, // outdated
                original_line: Some(7),
                diff_hunk: Some("@@".into()),
                body: "race condition".into(),
                user: user("alice", "User"),
                in_reply_to_id: None,
                pull_request_review_id: Some(99),
            },
            ReviewComment {
                id: 12,
                path: Some("b.rs".into()),
                line: Some(3),
                original_line: Some(3),
                diff_hunk: None,
                body: "nit".into(),
                user: user("coderabbitai", "Bot"),
                in_reply_to_id: None,
                pull_request_review_id: None,
            },
        ];
        let threads = vec![ReviewThread {
            id: "T1".into(),
            is_resolved: true,
            comment_ids: vec![11],
        }];

        let review = normalize(pr(), comments, vec![], vec![], threads);
        assert_eq!(review.comments.len(), 2);

        let c11 = &review.comments[0];
        assert_eq!(c11.line, Some(7)); // fell back to original_line
        assert!(c11.outdated);
        assert!(c11.is_resolved);
        assert_eq!(c11.thread_id.as_deref(), Some("T1"));
        assert!(!c11.is_bot);

        let c12 = &review.comments[1];
        assert!(!c12.outdated);
        assert!(!c12.is_resolved);
        assert!(c12.is_bot);

        assert_eq!(review.open_count(), 1); // only c12 is unresolved
    }

    #[test]
    fn normalize_drops_empty_review_summaries() {
        let reviews = vec![
            Review {
                id: 1,
                body: "".into(),
                state: "APPROVED".into(),
                user: user("bob", "User"),
            },
            Review {
                id: 2,
                body: "Please add a test.".into(),
                state: "CHANGES_REQUESTED".into(),
                user: user("bob", "User"),
            },
        ];
        let review = normalize(pr(), vec![], reviews, vec![], vec![]);
        assert_eq!(review.comments.len(), 1);
        assert_eq!(
            review.comments[0].kind,
            CommentKind::ReviewSummary {
                state: "CHANGES_REQUESTED".into()
            }
        );
    }

    #[test]
    fn agent_text_strips_only_for_bots() {
        let human = PrComment {
            id: 1,
            kind: CommentKind::Inline,
            author: "alice".into(),
            is_bot: false,
            path: None,
            line: None,
            outdated: false,
            diff_hunk: None,
            body: "<details>keep?</details>plain".into(),
            snippet: String::new(),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::Untriaged,
            local_note: None,
        };
        let mut bot = human.clone();
        bot.is_bot = true;
        assert!(human.agent_text().contains("<details>"));
        assert_eq!(bot.agent_text(), "plain");
    }
}
