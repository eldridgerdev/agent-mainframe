//! GitHub access via the `gh` CLI.
//!
//! A reusable tool-manager (in the spirit of [`TmuxManager`], [`WorktreeManager`],
//! and [`ClaudeLauncher`]) wrapping the GitHub CLI: preconditions (gh installed,
//! authenticated) plus typed PR queries. All work happens in Rust here — no agent
//! tokens are spent. Keep this layer feature-agnostic; feature-specific logic
//! (e.g. PR comment review) should live in its own module and call into this one.
//!
//! [`TmuxManager`]: crate::tmux::TmuxManager
//! [`WorktreeManager`]: crate::worktree::WorktreeManager
//! [`ClaudeLauncher`]: crate::claude::ClaudeLauncher

// This is a reusable layer landed ahead of its first consumer (the PR
// comment-review UI). Until that wires in, the public API is exercised only by
// unit tests.
#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Static-method wrapper around the `gh` CLI. Add general GitHub operations
/// here (PRs, issues, reviews, repo metadata) so they can be reused across
/// features rather than coupled to any one of them.
pub struct GhCli;

/// A resolved pull request, enough to drive every subsequent `gh` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRef {
    pub number: u32,
    pub head_sha: String,
    pub url: String,
    pub owner: String,
    pub repo: String,
    /// The PR's head branch name (GitHub's `headRefName`). Used to warn when
    /// the feature's checked-out branch doesn't match the PR being triaged —
    /// e.g. a manually-picked PR (`G`/`g`/`#`) unrelated to the current
    /// worktree. `#[serde(default)]` so pre-existing `pr_review_cache` rows
    /// (written before this field existed) still deserialize, just with an
    /// empty string (treated as "unknown", never flagged as a mismatch).
    #[serde(default)]
    pub head_ref: String,
}

/// Outcome of trying to resolve the PR for a branch. We distinguish "no PR for
/// this branch" (a normal state that should offer the manual-number override)
/// from hard errors (no remote, network) so the UI can react appropriately.
#[derive(Debug)]
pub enum PrResolution {
    Found(PrRef),
    NoPrForBranch,
}

/// One row in the PR picker — the lightweight metadata `gh pr list` returns, no
/// head SHA yet (that's resolved on selection via [`GhCli::fetch_pr_by_number`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PrListEntry {
    pub number: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_author_login")]
    pub author: String,
    #[serde(default, rename = "headRefName")]
    pub head_ref: String,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: String,
    #[serde(default, rename = "isDraft")]
    pub is_draft: bool,
    /// `OPEN`, `CLOSED`, or `MERGED`.
    #[serde(default)]
    pub state: String,
}

/// The PR selected by `gh pr view` from the local branch's tracking/push
/// configuration. We inspect its state so a closed predecessor is not restored
/// when the branch is reused before its successor PR is opened.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ResolvedPrCandidate {
    number: u32,
    #[serde(rename = "headRefOid")]
    head_sha: String,
    url: String,
    /// `OPEN`, `CLOSED`, or `MERGED`.
    #[serde(default)]
    state: String,
    #[serde(default, rename = "headRefName")]
    head_ref: String,
}

/// `gh pr list` nests the author under `{ "login": ... }`; flatten it to the
/// login string (empty when GitHub omits the user, e.g. a deleted account).
fn deserialize_author_login<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Author {
        #[serde(default)]
        login: String,
    }
    let author = Option::<Author>::deserialize(deserializer)?;
    Ok(author.map(|a| a.login).unwrap_or_default())
}

/// A GitHub account as embedded in comment/review payloads. `kind` is GitHub's
/// `type` field — `"User"`, `"Bot"`, `"Organization"`. We expose [`is_bot`] so
/// callers don't depend on the exact string.
///
/// [`is_bot`]: GhUser::is_bot
#[derive(Debug, Clone, Deserialize)]
pub struct GhUser {
    #[serde(default)]
    pub login: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

impl GhUser {
    /// Whether this account is a bot. Matches GitHub's `type == "Bot"` and the
    /// `name[bot]` login convention some apps (CodeRabbit, Copilot) use.
    pub fn is_bot(&self) -> bool {
        self.kind.eq_ignore_ascii_case("bot") || self.login.ends_with("[bot]")
    }
}

/// A raw inline review comment (`gh api .../pulls/{n}/comments`). Anchored to a
/// file/line, carries the `diff_hunk` GitHub provides for free, and links into
/// a thread via `in_reply_to_id`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    #[serde(default)]
    pub path: Option<String>,
    /// Current line in the diff; `None` when the comment is outdated.
    #[serde(default)]
    pub line: Option<u32>,
    /// Line the comment was originally left on (survives outdating).
    #[serde(default)]
    pub original_line: Option<u32>,
    /// Which side of the diff `line` addresses (`"RIGHT"` for the current
    /// file, `"LEFT"` for the base file).
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub diff_hunk: Option<String>,
    /// What the comment is anchored to: `"line"` (the default) or `"file"` for a
    /// comment left on the whole file. File-level comments carry the entire file
    /// diff as their `diff_hunk`, so this drives hunk suppression.
    #[serde(default)]
    pub subject_type: Option<String>,
    #[serde(default)]
    pub body: String,
    pub user: GhUser,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
    #[serde(default)]
    pub pull_request_review_id: Option<u64>,
}

/// A raw PR review summary (`gh api .../pulls/{n}/reviews`): the top-level body
/// attached to an Approve / Request-changes / Comment action.
#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    #[serde(default)]
    pub state: String,
    pub user: GhUser,
}

/// A raw issue/PR conversation comment (`gh api .../issues/{n}/comments`): not
/// anchored to any code line.
#[derive(Debug, Clone, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    pub user: GhUser,
}

/// A review thread's resolution state, from GraphQL (REST can't report this).
/// Maps each member comment's `databaseId` (== the REST `id`) to the thread.
#[derive(Debug, Clone)]
pub struct ReviewThread {
    /// GraphQL node id, used to resolve the thread later.
    pub id: String,
    pub is_resolved: bool,
    pub comment_ids: Vec<u64>,
}

/// One inline comment to post as part of a PR review (see
/// [`GhCli::create_review`]). `line` is the file line number; `side` is
/// `"RIGHT"` for the current file or `"LEFT"` for the base file. `start_line` /
/// `start_side`, when set, make this a multi-line comment spanning
/// `start_line`..`line`, matching the GitHub create-review API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrReviewComment {
    pub path: String,
    pub line: u32,
    pub side: &'static str,
    /// First line of a multi-line comment span. `None` for a single-line comment.
    pub start_line: Option<u32>,
    /// Side of `start_line` (`"RIGHT"` / `"LEFT"`). `None` for a single-line
    /// comment.
    pub start_side: Option<&'static str>,
    pub body: String,
}

/// A whole-file PR review comment (`subject_type: "file"`, see
/// [`GhCli::create_file_comment`]) — no line to anchor to, so it attaches to
/// the file itself rather than a diff row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFileComment {
    pub path: String,
    pub body: String,
}

/// Identity returned by GitHub after creating a PR review. Callers use the
/// review id to associate the subsequently-fetched inline comments with the
/// local findings that produced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatedPrReview {
    pub id: u64,
}

#[derive(Deserialize)]
struct CreatedPrReviewResponse {
    id: u64,
}

impl GhCli {
    /// Check that a working `gh` binary is on PATH. Mirrors
    /// `TmuxManager::check_available` / `ClaudeLauncher::check_available`.
    pub fn check_available() -> Result<()> {
        let output = Command::new("gh").arg("--version").output().context(
            "GitHub CLI (`gh`) not found. Install it from https://cli.github.com to use PR Triage.",
        )?;
        if !output.status.success() {
            bail!("GitHub CLI (`gh`) is not working correctly. Try `gh --version`.");
        }
        Ok(())
    }

    /// Check that `gh` is authenticated. We trust the exit code rather than
    /// parsing the human-readable text, which changes between versions.
    pub fn check_auth() -> Result<()> {
        let output = Command::new("gh")
            .args(["auth", "status"])
            .output()
            .context("Failed to run `gh auth status`.")?;
        if !output.status.success() {
            bail!(
                "`gh` is not authenticated. Run `! gh auth login` to sign in (login is interactive)."
            );
        }
        Ok(())
    }

    /// Resolve the open PR for the current branch in `workdir`.
    ///
    /// Returns `NoPrForBranch` when the repo has a GitHub remote but no open PR
    /// for the branch (caller should offer the manual-number override). Returns
    /// an error for missing-remote / network / other failures.
    pub fn resolve_pr(workdir: &Path) -> Result<PrResolution> {
        let output = Command::new("gh")
            .args([
                "pr",
                "view",
                "--json",
                "number,headRefOid,url,state,headRefName",
            ])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr view`.")?;

        if output.status.success() {
            return Ok(match parse_open_resolved_pr(&output.stdout)? {
                Some(pr) => PrResolution::Found(pr),
                None => PrResolution::NoPrForBranch,
            });
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        match classify_pr_view_error(&stderr) {
            PrViewError::NoPr => Ok(PrResolution::NoPrForBranch),
            PrViewError::NoRemote => bail!(
                "No GitHub remote found for this repository. PR Triage needs a GitHub-hosted repo."
            ),
            PrViewError::Other => bail!("`gh pr view` failed: {}", stderr.trim()),
        }
    }

    /// Whether the authenticated `gh` user authored PR `pr_number` — i.e.
    /// posting an approve / request-changes review would be a *self-review*,
    /// which GitHub rejects. Two lightweight `gh` calls (the PR's author login
    /// and the current user's login), compared case-insensitively. Any `gh`
    /// failure bubbles up so callers fall back to the always-valid `COMMENT`
    /// event rather than risk a 422 that discards the whole review.
    pub fn is_self_review(workdir: &Path, pr_number: u32) -> Result<bool> {
        let author = Self::gh_stdout(
            workdir,
            &[
                "pr",
                "view",
                &pr_number.to_string(),
                "--json",
                "author",
                "-q",
                ".author.login",
            ],
        )?;
        let me = Self::current_user(workdir)?;
        Ok(!author.is_empty() && author.eq_ignore_ascii_case(&me))
    }

    /// Resolve the authenticated `gh` user's login. Cheap (one `gh api` call),
    /// but callers driving UI (e.g. the PR picker) should memoize this for the
    /// session rather than re-resolving on every render.
    pub fn current_user(workdir: &Path) -> Result<String> {
        Self::gh_stdout(workdir, &["api", "user", "-q", ".login"])
    }

    /// Run `gh <args>` in `workdir` and return trimmed stdout, erroring on a
    /// non-zero exit.
    fn gh_stdout(workdir: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("gh")
            .args(args)
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh`.")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("`gh {}` failed: {}", args.join(" "), stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Resolve a PR by explicit number (manual override). Used when the branch
    /// has no associated PR or the user wants to review a different one.
    pub fn fetch_pr_by_number(workdir: &Path, number: u32) -> Result<PrRef> {
        let output = Command::new("gh")
            .args([
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "number,headRefOid,url,headRefName",
            ])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr view`.")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Could not load PR #{number}: {}", stderr.trim());
        }
        parse_pr_json(&output.stdout)
    }

    /// Fetch the PR's unified diff (`gh pr diff <number>`), plain patch text —
    /// not `--json`. Used as the AI reviewer's input; zero agent tokens by
    /// itself (one `gh` call). Only the whole string's leading/trailing
    /// whitespace is trimmed ([`Self::gh_stdout`]); interior diff content is
    /// untouched.
    pub fn pr_diff(workdir: &Path, number: u32) -> Result<String> {
        Self::gh_stdout(workdir, &["pr", "diff", &number.to_string()])
    }

    /// List the repository's pull requests for the PR picker. `include_closed`
    /// switches between open-only (`--state open`, the default) and everything
    /// (`--state all`, i.e. open + closed + merged). Newest-updated first.
    /// Zero agent tokens (one `gh pr list` call).
    pub fn list_prs(workdir: &Path, include_closed: bool) -> Result<Vec<PrListEntry>> {
        let state = if include_closed { "all" } else { "open" };
        let output = Command::new("gh")
            .args([
                "pr",
                "list",
                "--state",
                state,
                "--limit",
                "100",
                "--json",
                "number,title,author,headRefName,updatedAt,isDraft,state",
            ])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr list`.")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("`gh pr list` failed: {}", stderr.trim());
        }
        parse_pr_list_json(&output.stdout)
    }

    /// List the repository's most-recently-updated merged/closed pull
    /// requests, for the review-memory lookback bootstrap (Epic E): it needs
    /// PR history, not open work in progress. `gh`'s `--state closed` filter
    /// already covers both closed-without-merge and merged PRs (a merged
    /// PR's underlying state is `CLOSED`), so no client-side filtering is
    /// needed. `limit` bounds how many to fetch. Zero agent tokens (one
    /// `gh pr list` call).
    pub fn list_recent_closed_prs(workdir: &Path, limit: u32) -> Result<Vec<PrListEntry>> {
        let output = Command::new("gh")
            .args([
                "pr",
                "list",
                "--state",
                "closed",
                "--limit",
                &limit.to_string(),
                "--json",
                "number,title,author,headRefName,updatedAt,isDraft,state",
            ])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr list`.")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("`gh pr list` failed: {}", stderr.trim());
        }
        parse_pr_list_json(&output.stdout)
    }

    /// Inline review comments for a PR (file/line-anchored), all pages.
    pub fn pr_review_comments(workdir: &Path, number: u32) -> Result<Vec<ReviewComment>> {
        fetch_paginated(
            workdir,
            &format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments"),
        )
    }

    /// Top-level review summaries (Approve / Request-changes / Comment), all pages.
    pub fn pr_reviews(workdir: &Path, number: u32) -> Result<Vec<Review>> {
        fetch_paginated(
            workdir,
            &format!("repos/{{owner}}/{{repo}}/pulls/{number}/reviews"),
        )
    }

    /// Conversation comments on the PR's issue timeline, all pages.
    pub fn issue_comments(workdir: &Path, number: u32) -> Result<Vec<IssueComment>> {
        fetch_paginated(
            workdir,
            &format!("repos/{{owner}}/{{repo}}/issues/{number}/comments"),
        )
    }

    /// Every open PR in one repository, with each one's unresolved
    /// review-thread count, in a single query.
    ///
    /// This replaces a `gh pr view` *per feature* plus a thread query per PR.
    /// Cost now scales with the number of **repositories** on the dashboard
    /// rather than the number of features, which is what made the badge sweep
    /// able to exhaust an account's hourly GraphQL budget: 34 features cost ~68
    /// points a sweep where the repositories behind them cost a handful.
    ///
    /// Callers match a feature's branch against [`OpenPr::head_ref`].
    ///
    /// **Fork limitation.** This asks the repository `origin` points at. When a
    /// worktree's `origin` is a fork and the PR lives on the upstream
    /// repository, the PR is not in this result and the feature shows no badge.
    /// `gh pr view` resolved that case because it knows the base repo. Covering
    /// it means either pulling every open PR of the upstream (unbounded on a
    /// busy project) or going back to a query per branch, which is the cost
    /// this replaced — so it is deliberately left out rather than paid for on
    /// every sweep.
    pub fn open_prs(
        workdir: &Path,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<OpenPr>, GhGraphqlError> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut cmd = Command::new("gh");
            cmd.args(["api", "graphql", "-f", &format!("query={OPEN_PRS_QUERY}")])
                .args(["-F", &format!("owner={owner}")])
                .args(["-F", &format!("repo={repo}")])
                .current_dir(workdir);
            if let Some(c) = &cursor {
                cmd.args(["-F", &format!("cursor={c}")]);
            }
            let output = cmd.output().map_err(|e| {
                GhGraphqlError::Failed(format!("failed to run `gh api graphql`: {e}"))
            })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if is_rate_limited(&stdout) || is_rate_limited(&stderr) {
                return Err(GhGraphqlError::RateLimited);
            }
            if !output.status.success() {
                return Err(GhGraphqlError::Failed(format!(
                    "`gh api graphql` (open PRs) failed: {}",
                    stderr.trim()
                )));
            }

            let (mut page, next) = parse_open_prs_page(&output.stdout)
                .map_err(|e| GhGraphqlError::Failed(e.to_string()))?;
            all.append(&mut page);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(all)
    }

    /// Review-thread resolution state via GraphQL. REST can't report whether a
    /// thread is resolved, so this maps each member comment id to its thread.
    pub fn review_threads(
        workdir: &Path,
        owner: &str,
        repo: &str,
        number: u32,
    ) -> Result<Vec<ReviewThread>> {
        let mut threads = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut cmd = Command::new("gh");
            cmd.args([
                "api",
                "graphql",
                "-f",
                &format!("query={REVIEW_THREADS_QUERY}"),
            ])
            .args(["-F", &format!("owner={owner}")])
            .args(["-F", &format!("repo={repo}")])
            .args(["-F", &format!("pr={number}")])
            .current_dir(workdir);
            if let Some(c) = &cursor {
                cmd.args(["-F", &format!("cursor={c}")]);
            }
            let output = cmd.output().context("Failed to run `gh api graphql`.")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!(
                    "`gh api graphql` (review threads) failed: {}",
                    stderr.trim()
                );
            }
            let (mut page, next) = parse_review_threads_page(&output.stdout)?;
            threads.append(&mut page);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(threads)
    }

    /// Post a review on `pr` with a summary `body` and any inline `comments`,
    /// pinned to the PR head commit. `event` is the GitHub review action —
    /// `"COMMENT"`, `"REQUEST_CHANGES"`, or `"APPROVE"` (use `"COMMENT"` for a
    /// self-review, since GitHub forbids approving / requesting changes on your
    /// own PR).
    ///
    /// Best-effort by contract: GitHub rejects the *entire* review (HTTP 422) if
    /// any one inline comment points at a line outside the PR diff, so callers
    /// should treat an error as "couldn't post" and fall back to their own
    /// record (e.g. the local feedback file) rather than assume partial success.
    pub fn create_review(
        workdir: &Path,
        pr: &PrRef,
        body: &str,
        event: &str,
        comments: &[PrReviewComment],
    ) -> Result<CreatedPrReview> {
        let payload = build_review_request_json(&pr.head_sha, body, event, comments);
        let endpoint = format!("repos/{}/{}/pulls/{}/reviews", pr.owner, pr.repo, pr.number);

        let mut child = Command::new("gh")
            .args(["api", "--method", "POST", &endpoint, "--input", "-"])
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to run `gh api` to post the PR review.")?;
        child
            .stdin
            .take()
            .context("Failed to open stdin for `gh api`.")?
            .write_all(payload.to_string().as_bytes())
            .context("Failed to send the PR review payload to `gh api`.")?;
        let output = child
            .wait_with_output()
            .context("Failed to run `gh api` to post the PR review.")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if is_missing_write_scope(&stderr) {
                bail!(
                    "Posting to GitHub needs the `repo` scope. Run `! gh auth refresh -s repo` and try again."
                );
            }
            if is_review_rejected_entity_error(&stderr) {
                bail!(
                    "GitHub rejected the review (422) — most likely one of the inline comments \
                     no longer lines up with the current diff (the PR moved since the AI review \
                     ran). Refresh (r) and re-run the AI review (A), or skip the stale finding \
                     and try W again. Raw: {}",
                    stderr.trim()
                );
            }
            bail!("`gh api` (create review) failed: {}", stderr.trim());
        }
        let response: CreatedPrReviewResponse = serde_json::from_slice(&output.stdout)
            .context("Failed to parse the created GitHub review response.")?;
        Ok(CreatedPrReview { id: response.id })
    }

    /// Post a whole-file review comment (`subject_type: "file"`), pinned to the
    /// PR head commit. GitHub's batch `create_review` endpoint has no
    /// file-level comment support in its `comments` array (only `line` /
    /// `start_line` anchoring), and there's no way to attach a comment to an
    /// already-created review after the fact — so this goes through the
    /// single review-comment endpoint instead and posts immediately as its
    /// own comment, not bundled into a `create_review` review object.
    pub fn create_file_comment(workdir: &Path, pr: &PrRef, path: &str, body: &str) -> Result<()> {
        let mut cmd = Command::new("gh");
        cmd.args(["api", "--method", "POST"])
            .arg(format!(
                "repos/{}/{}/pulls/{}/comments",
                pr.owner, pr.repo, pr.number
            ))
            .args(["-f", &format!("commit_id={}", pr.head_sha)])
            .args(["-f", &format!("path={path}")])
            .args(["-f", "subject_type=file"])
            .args(["-f", &format!("body={body}")])
            .current_dir(workdir);
        run_write(cmd, "post file-level comment")
    }

    /// Post a reply into an existing inline review thread. `root_comment_id` is
    /// the **top-level** comment that started the thread (GitHub's replies
    /// endpoint rejects replying to a reply); callers reply to
    /// `in_reply_to_id.unwrap_or(id)`. The reply posts as the authenticated
    /// `gh` user.
    pub fn reply_to_review_comment(
        workdir: &Path,
        owner: &str,
        repo: &str,
        number: u32,
        root_comment_id: u64,
        body: &str,
    ) -> Result<()> {
        let mut cmd = Command::new("gh");
        cmd.args(["api", "--method", "POST"])
            .arg(format!(
                "repos/{owner}/{repo}/pulls/{number}/comments/{root_comment_id}/replies"
            ))
            .args(["-f", &format!("body={body}")])
            .current_dir(workdir);
        run_write(cmd, "post inline reply")
    }

    /// Post a top-level conversation comment on the PR's issue timeline. Used to
    /// reply to comments that have no inline thread (conversation comments,
    /// review summaries). Posts as the authenticated `gh` user.
    pub fn post_issue_comment(
        workdir: &Path,
        owner: &str,
        repo: &str,
        number: u32,
        body: &str,
    ) -> Result<()> {
        let mut cmd = Command::new("gh");
        cmd.args(["api", "--method", "POST"])
            .arg(format!("repos/{owner}/{repo}/issues/{number}/comments"))
            .args(["-f", &format!("body={body}")])
            .current_dir(workdir);
        run_write(cmd, "post conversation comment")
    }

    /// Resolve or unresolve a review thread via GraphQL (REST can't do this),
    /// returning the thread's resulting `isResolved`. `thread_id` is the GraphQL
    /// node id captured in [`ReviewThread::id`]. Runs as the authenticated `gh`
    /// user and needs the `repo` scope — a first-write 403 maps to the same
    /// actionable `gh auth refresh -s repo` message as the reply path.
    pub fn set_thread_resolved(workdir: &Path, thread_id: &str, resolved: bool) -> Result<bool> {
        let mutation = if resolved {
            "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}"
        } else {
            "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{isResolved}}}"
        };
        let output = Command::new("gh")
            .args(["api", "graphql", "-f", &format!("query={mutation}")])
            .args(["-F", &format!("id={thread_id}")])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh api graphql` (resolve thread).")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if is_missing_write_scope(&stderr) {
                bail!(
                    "Resolving a thread needs the `repo` scope. Run `! gh auth refresh -s repo` and try again."
                );
            }
            bail!(
                "`gh api graphql` (resolve thread) failed: {}",
                stderr.trim()
            );
        }
        parse_thread_resolved(&output.stdout, resolved)
    }
}

/// Build the JSON request body for the GitHub create-review API. Omits an empty
/// `body` / `commit_id` and the `comments` array when there are none, so a
/// summary-only review is a valid request.
fn build_review_request_json(
    commit_id: &str,
    body: &str,
    event: &str,
    comments: &[PrReviewComment],
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if !commit_id.is_empty() {
        obj.insert("commit_id".into(), json!(commit_id));
    }
    if !body.is_empty() {
        obj.insert("body".into(), json!(body));
    }
    obj.insert("event".into(), json!(event));
    if !comments.is_empty() {
        let arr: Vec<serde_json::Value> = comments
            .iter()
            .map(|c| {
                let mut comment = serde_json::Map::new();
                comment.insert("path".into(), json!(c.path));
                comment.insert("line".into(), json!(c.line));
                comment.insert("side".into(), json!(c.side));
                // A multi-line comment carries the span's start; omitted for a
                // single-line comment.
                if let Some(start_line) = c.start_line {
                    comment.insert("start_line".into(), json!(start_line));
                }
                if let Some(start_side) = c.start_side {
                    comment.insert("start_side".into(), json!(start_side));
                }
                comment.insert("body".into(), json!(c.body));
                serde_json::Value::Object(comment)
            })
            .collect();
        obj.insert("comments".into(), json!(arr));
    }
    serde_json::Value::Object(obj)
}

/// Run a `gh` write command, mapping a missing-`repo`-scope failure (the common
/// first-write 403) to an actionable message instead of a raw HTTP error.
fn run_write(mut cmd: Command, what: &str) -> Result<()> {
    let output = cmd
        .output()
        .with_context(|| format!("Failed to run `gh api` ({what})."))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_missing_write_scope(&stderr) {
        bail!(
            "Posting to GitHub needs the `repo` scope. Run `! gh auth refresh -s repo` and try again."
        );
    }
    bail!("`gh api` ({what}) failed: {}", stderr.trim());
}

/// Whether a `gh` failure looks like a missing-write-scope / 403, so we can
/// point the user at `gh auth refresh -s repo` rather than show a raw error.
fn is_missing_write_scope(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("http 403") || (s.contains("403") && s.contains("scope")) || s.contains("must have")
}

/// Whether a `create_review` failure is GitHub's 422 Unprocessable Entity —
/// its documented response when any inline comment in the review doesn't
/// land inside the PR's current diff. `gh api`'s stderr for this case is
/// just a terse status line with no detail on which comment is at fault, so
/// this drives a friendlier, actionable message instead of passing it
/// through verbatim.
fn is_review_rejected_entity_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("422") || s.contains("unprocessable entity")
}

/// Run `gh api --paginate --slurp <endpoint>` and flatten the result.
///
/// `--slurp` wraps paginated array responses as an array *of pages*
/// (`[[...],[...]]`), so we deserialize `Vec<Vec<T>>` and flatten. `{owner}` /
/// `{repo}` placeholders in `endpoint` are expanded by `gh` from the repo in
/// `workdir`.
fn fetch_paginated<T: DeserializeOwned>(workdir: &Path, endpoint: &str) -> Result<Vec<T>> {
    let output = Command::new("gh")
        .args(["api", "--paginate", "--slurp", endpoint])
        .current_dir(workdir)
        .output()
        .with_context(|| format!("Failed to run `gh api {endpoint}`."))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`gh api {endpoint}` failed: {}", stderr.trim());
    }
    let pages: Vec<Vec<T>> = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Failed to parse `gh api {endpoint}` output."))?;
    Ok(pages.into_iter().flatten().collect())
}

/// Parse one page of the review-threads GraphQL response into
/// `(threads, next_cursor)`.
/// PR Triage's review-thread query: thread resolution plus the comment ids
/// that map each comment to its thread.
const REVIEW_THREADS_QUERY: &str = "
    query($owner:String!,$repo:String!,$pr:Int!,$cursor:String){
      repository(owner:$owner,name:$repo){
        pullRequest(number:$pr){
          reviewThreads(first:100,after:$cursor){
            pageInfo{hasNextPage endCursor}
            nodes{id isResolved comments(first:100){nodes{databaseId}}}
          }
        }
      }
    }";

/// The dashboard badge's per-repository query.
///
/// Written as a real multi-line string, not with `\` continuations. A
/// continuation eats the newline *and* the following indentation, so a line
/// ending in one field name and the next beginning with another silently fuses
/// them — which is exactly how this query first shipped, asking GitHub for
/// `headRefOidreviewThreads` and failing every call. GraphQL is
/// whitespace-insensitive, so real newlines cost nothing.
///
/// `first:50` rather than 100: cost scales with nodes actually returned, and a
/// repository with 50 open PRs already returns their threads too. Pagination
/// covers the rest.
const OPEN_PRS_QUERY: &str = "
            query($owner:String!,$repo:String!,$cursor:String){
              repository(owner:$owner,name:$repo){
                pullRequests(states:OPEN,first:50,after:$cursor){
                  pageInfo{hasNextPage endCursor}
                  nodes{
                    number
                    headRefName
                    headRefOid
                    reviewThreads(first:100){nodes{isResolved}}
                  }
                }
              }
            }";

/// One open pull request, as the dashboard badge needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: u32,
    /// GitHub's `headRefName` — the branch the PR is *from*, which is what a
    /// feature's branch is matched against.
    pub head_ref: String,
    pub head_sha: String,
    pub unresolved_threads: usize,
}

/// Resolve `owner/repo` from the repository's `origin` remote.
///
/// Deliberately local: this is `git remote get-url`, not an API call. The
/// previous route to owner/repo was parsing it back out of a PR url returned by
/// `gh pr view`, which meant spending an API call *per feature* just to learn
/// something every worktree of a repo already agrees on.
///
/// Handles both remote spellings: `https://host/owner/repo(.git)` and
/// `git@host:owner/repo(.git)`.
pub fn owner_repo_from_remote(workdir: &Path) -> Option<(String, String)> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_remote_owner_repo(String::from_utf8_lossy(&output.stdout).trim())
}

/// Pull `owner/repo` out of a git remote url, in either spelling.
fn parse_remote_owner_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    // The presence of a scheme is what separates the two spellings. Testing
    // for `://` rather than for `:` matters: `ssh://git@host/owner/repo` has
    // both, and treating it as scp-style yields the host as the owner.
    let path = match url.split_once("://") {
        // Any scheme (https, ssh, git). Whatever precedes the first `/` is the
        // host, with userinfo already attached to it.
        Some((_scheme, rest)) => rest.split_once('/')?.1.to_string(),
        // scp-style: git@host:owner/repo(.git)
        None => url.split_once(':')?.1.to_string(),
    };

    let path = path.trim_start_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let owner = segments.next()?.to_string();
    let repo = segments.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

/// Sum unresolved threads on one PR node, tolerating a missing connection.
fn unresolved_in_node(node: &serde_json::Value) -> usize {
    node["reviewThreads"]["nodes"]
        .as_array()
        .map(|threads| {
            threads
                .iter()
                .filter(|t| !t["isResolved"].as_bool().unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

/// Parse one page of the repository-wide open-PR query.
fn parse_open_prs_page(stdout: &[u8]) -> Result<(Vec<OpenPr>, Option<String>)> {
    let v: serde_json::Value =
        serde_json::from_slice(stdout).context("Failed to parse open-PRs GraphQL JSON.")?;
    let prs = &v["data"]["repository"]["pullRequests"];

    let open = prs["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    Some(OpenPr {
                        number: u32::try_from(n["number"].as_u64()?).ok()?,
                        head_ref: n["headRefName"].as_str()?.to_string(),
                        head_sha: n["headRefOid"].as_str().unwrap_or_default().to_string(),
                        unresolved_threads: unresolved_in_node(n),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let next = if prs["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
        prs["pageInfo"]["endCursor"].as_str().map(|s| s.to_string())
    } else {
        None
    };
    Ok((open, next))
}

/// Why a GraphQL call failed, distinguishing the one cause worth reacting to.
///
/// A depleted point budget is not a transient error: every subsequent call in
/// the same window fails the same way, so a caller on a timer has to stop
/// rather than retry. Everything else stays an opaque string, since the
/// callers only report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhGraphqlError {
    /// GitHub's hourly GraphQL point budget is exhausted.
    RateLimited,
    Failed(String),
}

impl std::fmt::Display for GhGraphqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GhGraphqlError::RateLimited => {
                write!(f, "GitHub's hourly GraphQL rate limit is exhausted")
            }
            GhGraphqlError::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

/// Whether a `gh` response reports an exhausted rate limit.
///
/// Matched on text rather than a parsed shape because GitHub reports this
/// several different ways, and an exhausted budget must be recognised through
/// all of them. Observed from a real depleted account:
///
/// ```text
/// {"errors":[{"type":"RATE_LIMIT","code":"graphql_rate_limit",
///   "message":"API rate limit already exceeded for user ID 1."}]}
/// ```
///
/// Note `RATE_LIMIT`, not the `RATE_LIMITED` the schema's enum suggests, and
/// note it arrives with HTTP 200 — so neither the exit status nor a guess at
/// the type name is enough on its own. `already exceeded` also differs from the
/// REST wording (`API rate limit exceeded`), which is why the last clause
/// matches the two words separately rather than a fixed phrase.
fn is_rate_limited(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("graphql_rate_limit")
        || lowered.contains("rate_limited")
        || lowered.contains("\"rate_limit\"")
        || lowered.contains("secondary rate limit")
        || (lowered.contains("rate limit") && lowered.contains("exceeded"))
}

fn parse_review_threads_page(stdout: &[u8]) -> Result<(Vec<ReviewThread>, Option<String>)> {
    let v: serde_json::Value =
        serde_json::from_slice(stdout).context("Failed to parse review-threads GraphQL JSON.")?;
    let rt = &v["data"]["repository"]["pullRequest"]["reviewThreads"];

    let threads = rt["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    Some(ReviewThread {
                        id: n["id"].as_str()?.to_string(),
                        is_resolved: n["isResolved"].as_bool().unwrap_or(false),
                        comment_ids: n["comments"]["nodes"]
                            .as_array()
                            .map(|cs| cs.iter().filter_map(|c| c["databaseId"].as_u64()).collect())
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let next = if rt["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
        rt["pageInfo"]["endCursor"].as_str().map(|s| s.to_string())
    } else {
        None
    };
    Ok((threads, next))
}

/// Parse the `resolveReviewThread` / `unresolveReviewThread` mutation response
/// into the thread's resulting `isResolved`. `requested` selects which mutation
/// field to read; GraphQL errors (returned with a 200) are surfaced.
fn parse_thread_resolved(stdout: &[u8], requested: bool) -> Result<bool> {
    let v: serde_json::Value =
        serde_json::from_slice(stdout).context("Failed to parse resolve-thread GraphQL JSON.")?;

    let field = if requested {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    if let Some(b) = v["data"][field]["thread"]["isResolved"].as_bool() {
        return Ok(b);
    }
    if let Some(errs) = v["errors"].as_array().filter(|e| !e.is_empty()) {
        let msg = errs
            .iter()
            .filter_map(|e| e["message"].as_str())
            .collect::<Vec<_>>()
            .join("; ");
        bail!("GitHub rejected the resolve: {msg}");
    }
    bail!("resolve-thread response missing isResolved");
}

#[derive(Debug, PartialEq, Eq)]
enum PrViewError {
    NoPr,
    NoRemote,
    Other,
}

/// Classify `gh pr view` stderr into the failure modes we care about. Matching
/// is substring-based and lowercased to survive minor wording changes.
fn classify_pr_view_error(stderr: &str) -> PrViewError {
    let s = stderr.to_lowercase();
    if s.contains("no pull requests found") || s.contains("no open pull requests found") {
        PrViewError::NoPr
    } else if s.contains("no git remotes found")
        || s.contains("none of the git remotes")
        || s.contains("not a git repository")
    {
        PrViewError::NoRemote
    } else {
        PrViewError::Other
    }
}

/// Parse the `{number, headRefOid, url}` JSON from `gh pr view`, deriving
/// owner/repo from the PR URL (avoids a second `gh` call).
fn parse_pr_json(stdout: &[u8]) -> Result<PrRef> {
    let v: serde_json::Value =
        serde_json::from_slice(stdout).context("Failed to parse `gh pr view` JSON output.")?;

    let number = v
        .get("number")
        .and_then(|n| n.as_u64())
        .context("`gh pr view` output missing PR number.")? as u32;
    let head_sha = v
        .get("headRefOid")
        .and_then(|s| s.as_str())
        .context("`gh pr view` output missing headRefOid.")?
        .to_string();
    let url = v
        .get("url")
        .and_then(|s| s.as_str())
        .context("`gh pr view` output missing url.")?
        .to_string();
    let head_ref = v
        .get("headRefName")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string();

    let (owner, repo) = parse_owner_repo(&url)
        .with_context(|| format!("Could not parse owner/repo from PR url: {url}"))?;

    Ok(PrRef {
        number,
        head_sha,
        url,
        owner,
        repo,
        head_ref,
    })
}

/// Accept the PR selected by `gh pr view` only while it is open. `gh` retains
/// the local branch's remote/tracking-aware selection semantics, while this
/// state check prevents a closed predecessor from being auto-restored.
fn parse_open_resolved_pr(stdout: &[u8]) -> Result<Option<PrRef>> {
    let candidate: ResolvedPrCandidate =
        serde_json::from_slice(stdout).context("Failed to parse `gh pr view` JSON output.")?;
    if !candidate.state.eq_ignore_ascii_case("open") {
        return Ok(None);
    }
    let (owner, repo) = parse_owner_repo(&candidate.url)
        .with_context(|| format!("Could not parse owner/repo from PR url: {}", candidate.url))?;
    Ok(Some(PrRef {
        number: candidate.number,
        head_sha: candidate.head_sha,
        url: candidate.url,
        owner,
        repo,
        head_ref: candidate.head_ref,
    }))
}

/// Parse the `gh pr list --json …` array into [`PrListEntry`] rows, sorted
/// newest-updated first (GitHub's order is not guaranteed across flags).
fn parse_pr_list_json(stdout: &[u8]) -> Result<Vec<PrListEntry>> {
    let mut entries: Vec<PrListEntry> =
        serde_json::from_slice(stdout).context("Failed to parse `gh pr list` JSON output.")?;
    // `updatedAt` is RFC 3339, so a lexical reverse sort is chronological.
    entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(entries)
}

/// Extract `(owner, repo)` from a GitHub PR URL like
/// `https://github.com/owner/repo/pull/123`. Works for GHES hosts too since we
/// key off the `/pull/` segment rather than the host.
fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let mut segments = after_scheme.split('/');
    let _host = segments.next()?;
    let owner = segments.next()?;
    let repo = segments.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    /// Every identifier in a GraphQL document, in order.
    fn graphql_identifiers(query: &str) -> Vec<String> {
        query
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .filter(|token| !token.is_empty() && !token.starts_with(|c: char| c.is_ascii_digit()))
            .map(str::to_string)
            .collect()
    }

    /// The query has to be *checked*, not just its parser.
    ///
    /// This module's tests exercised `parse_open_prs_page` against handcrafted
    /// JSON and passed while the query itself was malformed: a `\` line
    /// continuation had fused two field names into `headRefOidreviewThreads`,
    /// so GitHub rejected every call and the badge silently reported nothing.
    /// Nothing that ran offline could see it, because nothing looked at the
    /// query string.
    ///
    /// An unknown identifier is the signature of exactly that failure — two
    /// legal names concatenated make one illegal one — so the assertion is
    /// that the document contains no identifier this query has no business
    /// asking for.
    #[test]
    fn the_open_prs_query_names_every_field_as_its_own_token() {
        const EXPECTED: &[&str] = &[
            // operation + variables
            "query",
            "owner",
            "String",
            "repo",
            "cursor",
            // selection
            "repository",
            "name",
            "pullRequests",
            "states",
            "OPEN",
            "first",
            "after",
            "pageInfo",
            "hasNextPage",
            "endCursor",
            "nodes",
            "number",
            "headRefName",
            "headRefOid",
            "reviewThreads",
            "isResolved",
        ];

        let identifiers = graphql_identifiers(OPEN_PRS_QUERY);

        for field in [
            "number",
            "headRefName",
            "headRefOid",
            "reviewThreads",
            "isResolved",
            "hasNextPage",
            "endCursor",
        ] {
            assert!(
                identifiers.iter().any(|token| token == field),
                "`{field}` is missing or fused with a neighbour; query was:\n{OPEN_PRS_QUERY}"
            );
        }

        for token in &identifiers {
            assert!(
                EXPECTED.contains(&token.as_str()),
                "unexpected identifier `{token}` — two field names have probably \
                 run together; query was:\n{OPEN_PRS_QUERY}"
            );
        }
    }

    /// The same check for the review-thread query PR Triage uses. Its line
    /// breaks happen to fall after braces today, so it survived the bug above
    /// by luck rather than by design.
    #[test]
    fn the_review_threads_query_names_every_field_as_its_own_token() {
        const EXPECTED: &[&str] = &[
            "query",
            "owner",
            "String",
            "repo",
            "pr",
            "Int",
            "cursor",
            "repository",
            "name",
            "pullRequest",
            "number",
            "reviewThreads",
            "first",
            "after",
            "pageInfo",
            "hasNextPage",
            "endCursor",
            "nodes",
            "id",
            "isResolved",
            "comments",
            "databaseId",
        ];

        let identifiers = graphql_identifiers(REVIEW_THREADS_QUERY);
        for field in ["id", "isResolved", "comments", "databaseId"] {
            assert!(
                identifiers.iter().any(|token| token == field),
                "`{field}` is missing or fused with a neighbour"
            );
        }
        for token in &identifiers {
            assert!(
                EXPECTED.contains(&token.as_str()),
                "unexpected identifier `{token}` — two field names have probably run together"
            );
        }
    }

    /// One query answers a whole repository: each PR carries the branch it is
    /// from and its own unresolved count, so a feature is matched locally
    /// instead of costing an API call of its own.
    #[test]
    fn the_open_prs_parser_reads_branch_sha_and_unresolved_count_per_pr() {
        let json = br#"{"data":{"repository":{"pullRequests":{
            "pageInfo":{"hasNextPage":false,"endCursor":null},
            "nodes":[
              {"number":546,"headRefName":"todo-plan","headRefOid":"abc1234",
               "reviewThreads":{"nodes":[{"isResolved":false},{"isResolved":true}]}},
              {"number":343,"headRefName":"fixture","headRefOid":"def5678",
               "reviewThreads":{"nodes":[]}}]}}}}"#;
        let (prs, next) = parse_open_prs_page(json).unwrap();
        assert_eq!(next, None);
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 546);
        assert_eq!(prs[0].head_ref, "todo-plan");
        assert_eq!(prs[0].head_sha, "abc1234");
        assert_eq!(prs[0].unresolved_threads, 1);
        assert_eq!(prs[1].unresolved_threads, 0);
    }

    /// A thread with no `isResolved` counts as unresolved, matching
    /// `parse_review_threads_page`'s `unwrap_or(false)`. Under-reporting work
    /// still to do is the worse failure.
    #[test]
    fn a_thread_missing_its_resolution_counts_as_unresolved() {
        let json = br#"{"data":{"repository":{"pullRequests":{"pageInfo":{"hasNextPage":false},
            "nodes":[{"number":1,"headRefName":"b","headRefOid":"s",
              "reviewThreads":{"nodes":[{},{"isResolved":true}]}}]}}}}"#;
        let (prs, _) = parse_open_prs_page(json).unwrap();
        assert_eq!(prs[0].unresolved_threads, 1);
    }

    /// Pagination carries the cursor, so a repository with more than 50 open
    /// PRs is read in full rather than truncated at the first page.
    #[test]
    fn the_open_prs_parser_reports_the_next_cursor() {
        let json = br#"{"data":{"repository":{"pullRequests":{
            "pageInfo":{"hasNextPage":true,"endCursor":"Y3Vyc29yOjUw"},
            "nodes":[]}}}}"#;
        let (prs, next) = parse_open_prs_page(json).unwrap();
        assert!(prs.is_empty());
        assert_eq!(next.as_deref(), Some("Y3Vyc29yOjUw"));
    }

    /// An empty or shape-drifted response is "no open PRs", not an error: the
    /// sweep degrades to clearing badges rather than failing the repository.
    #[test]
    fn an_unexpected_shape_reads_as_no_open_prs() {
        let (prs, next) = parse_open_prs_page(b"{}").unwrap();
        assert!(prs.is_empty());
        assert_eq!(next, None);
    }

    /// A PR missing `headRefName` cannot be matched to a feature branch, so it
    /// is dropped rather than matched against every feature with an empty
    /// branch name.
    #[test]
    fn a_pr_without_a_head_branch_is_skipped() {
        let json = br#"{"data":{"repository":{"pullRequests":{"pageInfo":{"hasNextPage":false},
            "nodes":[{"number":7,"headRefOid":"s","reviewThreads":{"nodes":[]}}]}}}}"#;
        let (prs, _) = parse_open_prs_page(json).unwrap();
        assert!(prs.is_empty());
    }

    /// Owner/repo comes from the git remote, in either spelling, so learning it
    /// costs no API call at all.
    #[test]
    fn owner_repo_is_read_from_either_remote_spelling() {
        for url in [
            "https://github.com/eldridgerdev/agent-mainframe.git",
            "https://github.com/eldridgerdev/agent-mainframe",
            "git@github.com:eldridgerdev/agent-mainframe.git",
            "ssh://git@github.com/eldridgerdev/agent-mainframe.git",
            "git@git.example.com:eldridgerdev/agent-mainframe",
        ] {
            assert_eq!(
                parse_remote_owner_repo(url),
                Some(("eldridgerdev".into(), "agent-mainframe".into())),
                "failed on {url}"
            );
        }
    }

    /// A remote that names no repository yields nothing, so the sweep reports
    /// "no PR" instead of querying a repo it cannot name.
    #[test]
    fn an_unusable_remote_yields_no_owner_repo() {
        for url in ["", "   ", "https://github.com/onlyowner", "not a url"] {
            assert_eq!(parse_remote_owner_repo(url), None, "failed on {url:?}");
        }
    }

    /// GraphQL reports a depleted budget with HTTP 200 and an `errors[].type`,
    /// so the exit status alone never catches it. REST's phrasing and the
    /// secondary limit have to match too, because `gh pr view` spends the same
    /// budget and fails its own way.
    #[test]
    fn rate_limit_detection_covers_how_gh_actually_reports_it() {
        // Captured verbatim from a real depleted account. Note `RATE_LIMIT`
        // (not the `RATE_LIMITED` the enum name suggests) and "already
        // exceeded" (not REST's "exceeded") — this case is why the matcher
        // does not key on one phrase.
        assert!(is_rate_limited(
            r#"{"errors":[{"type":"RATE_LIMIT","code":"graphql_rate_limit","message":"API rate limit already exceeded for user ID 72774132."}]}"#
        ));
        assert!(is_rate_limited(
            r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#
        ));
        assert!(is_rate_limited("API rate limit exceeded for user ID 1."));
        assert!(is_rate_limited("You have exceeded a secondary rate limit."));
        assert!(is_rate_limited("HTTP 403: rate limit exceeded"));
    }

    /// An ordinary failure must not be mistaken for a rate limit: that would
    /// pause PR badges for fifteen minutes over a typo'd repo name.
    #[test]
    fn ordinary_failures_are_not_read_as_rate_limits() {
        assert!(!is_rate_limited("Could not resolve to a Repository."));
        assert!(!is_rate_limited("no pull requests found for branch"));
        assert!(!is_rate_limited(""));
        // "rate limit" alone, without exhaustion, is documentation not failure.
        assert!(!is_rate_limited(
            "See the rate limit documentation for details."
        ));
    }
    use super::*;

    #[test]
    fn detects_missing_write_scope() {
        assert!(is_missing_write_scope(
            "gh: HTTP 403: Resource not accessible by integration"
        ));
        assert!(is_missing_write_scope(
            "403 your token has not been granted the required scopes"
        ));
        assert!(!is_missing_write_scope("HTTP 422: Validation Failed"));
        assert!(!is_missing_write_scope("could not resolve host github.com"));
    }

    #[test]
    fn detects_review_rejected_entity_error() {
        // The exact terse line `gh api` gives for this case (confirmed from
        // real use — the debug log showed exactly this).
        assert!(is_review_rejected_entity_error(
            "gh: Unprocessable Entity (HTTP 422)"
        ));
        assert!(is_review_rejected_entity_error(
            "HTTP 422: Validation Failed"
        ));
        assert!(!is_review_rejected_entity_error(
            "gh: HTTP 403: Resource not accessible by integration"
        ));
        assert!(!is_review_rejected_entity_error(
            "could not resolve host github.com"
        ));
    }

    #[test]
    fn parses_pr_list_and_sorts_newest_first() {
        let json = br#"[
            {"number":10,"title":"Old one","author":{"login":"alice"},"headRefName":"feat-a","updatedAt":"2026-01-01T00:00:00Z","isDraft":false,"state":"OPEN"},
            {"number":12,"title":"New one","author":{"login":"bob"},"headRefName":"feat-b","updatedAt":"2026-06-01T00:00:00Z","isDraft":true,"state":"OPEN"},
            {"number":11,"title":"Merged one","author":null,"headRefName":"feat-c","updatedAt":"2026-03-01T00:00:00Z","isDraft":false,"state":"MERGED"}
        ]"#;
        let entries = parse_pr_list_json(json).unwrap();
        // Sorted by updatedAt descending.
        assert_eq!(
            entries.iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![12, 11, 10]
        );
        // Author login is flattened; a null author degrades to empty.
        assert_eq!(entries[0].author, "bob");
        assert!(entries[0].is_draft);
        assert_eq!(entries[1].author, "");
        assert_eq!(entries[1].state, "MERGED");
    }

    #[test]
    fn parses_owner_repo_from_url() {
        assert_eq!(
            parse_owner_repo("https://github.com/eldridgerdev/agent-mainframe/pull/321"),
            Some(("eldridgerdev".to_string(), "agent-mainframe".to_string()))
        );
    }

    #[test]
    fn parses_owner_repo_from_ghes_url() {
        assert_eq!(
            parse_owner_repo("https://git.example.com/org/proj/pull/7"),
            Some(("org".to_string(), "proj".to_string()))
        );
    }

    #[test]
    fn owner_repo_rejects_incomplete_url() {
        assert_eq!(parse_owner_repo("https://github.com/onlyowner"), None);
    }

    #[test]
    fn parses_pr_json() {
        let json = br#"{"number":321,"headRefOid":"abc123","url":"https://github.com/o/r/pull/321","headRefName":"feature-x"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.number, 321);
        assert_eq!(pr.head_sha, "abc123");
        assert_eq!(pr.owner, "o");
        assert_eq!(pr.repo, "r");
        assert_eq!(pr.head_ref, "feature-x");
    }

    #[test]
    fn parses_pr_json_missing_head_ref_name_defaults_empty() {
        // Older cached rows / defensive parsing: a response without
        // `headRefName` shouldn't fail the whole parse, just leave the field
        // empty (never flagged as a branch mismatch — see `branch_mismatch`).
        let json =
            br#"{"number":321,"headRefOid":"abc123","url":"https://github.com/o/r/pull/321"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.head_ref, "");
    }

    #[test]
    fn detects_bots() {
        let by_type = GhUser {
            login: "coderabbitai".into(),
            kind: "Bot".into(),
        };
        let by_login = GhUser {
            login: "copilot[bot]".into(),
            kind: "User".into(),
        };
        let human = GhUser {
            login: "alice".into(),
            kind: "User".into(),
        };
        assert!(by_type.is_bot());
        assert!(by_login.is_bot());
        assert!(!human.is_bot());
    }

    #[test]
    fn deserializes_review_comment_with_missing_optionals() {
        // Outdated comment: line is null, no in_reply_to_id.
        let json = br#"[[{"id":1,"path":"a.rs","line":null,"original_line":7,
            "diff_hunk":"@@","body":"hi","user":{"login":"alice","type":"User"},
            "in_reply_to_id":null,"pull_request_review_id":42}]]"#;
        let pages: Vec<Vec<ReviewComment>> = serde_json::from_slice(json).unwrap();
        let flat: Vec<_> = pages.into_iter().flatten().collect();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].line, None);
        assert_eq!(flat[0].original_line, Some(7));
        assert_eq!(flat[0].pull_request_review_id, Some(42));
    }

    #[test]
    fn parses_review_threads_page() {
        let json = br#"{"data":{"repository":{"pullRequest":{"reviewThreads":{
            "pageInfo":{"hasNextPage":true,"endCursor":"CUR2"},
            "nodes":[
              {"id":"T1","isResolved":true,"comments":{"nodes":[{"databaseId":11},{"databaseId":12}]}},
              {"id":"T2","isResolved":false,"comments":{"nodes":[{"databaseId":13}]}}
            ]}}}}}"#;
        let (threads, next) = parse_review_threads_page(json).unwrap();
        assert_eq!(next.as_deref(), Some("CUR2"));
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, "T1");
        assert!(threads[0].is_resolved);
        assert_eq!(threads[0].comment_ids, vec![11, 12]);
        assert!(!threads[1].is_resolved);
    }

    #[test]
    fn review_threads_last_page_has_no_cursor() {
        let json = br#"{"data":{"repository":{"pullRequest":{"reviewThreads":{
            "pageInfo":{"hasNextPage":false,"endCursor":"X"},"nodes":[]}}}}}"#;
        let (threads, next) = parse_review_threads_page(json).unwrap();
        assert!(threads.is_empty());
        assert_eq!(next, None);
    }

    #[test]
    fn parses_resolved_thread_mutation() {
        let json = br#"{"data":{"resolveReviewThread":{"thread":{"isResolved":true}}}}"#;
        assert!(parse_thread_resolved(json, true).unwrap());

        let json = br#"{"data":{"unresolveReviewThread":{"thread":{"isResolved":false}}}}"#;
        assert!(!parse_thread_resolved(json, false).unwrap());
    }

    #[test]
    fn resolve_thread_surfaces_graphql_errors() {
        let json = br#"{"data":null,"errors":[{"message":"Could not resolve to a node."}]}"#;
        let err = parse_thread_resolved(json, true).unwrap_err().to_string();
        assert!(err.contains("Could not resolve to a node."));
    }

    #[test]
    fn classifies_no_pr() {
        assert_eq!(
            classify_pr_view_error("no pull requests found for branch \"foo\""),
            PrViewError::NoPr
        );
    }

    #[test]
    fn classifies_no_remote() {
        assert_eq!(
            classify_pr_view_error("no git remotes found"),
            PrViewError::NoRemote
        );
    }

    #[test]
    fn classifies_other() {
        assert_eq!(
            classify_pr_view_error("HTTP 500 something broke"),
            PrViewError::Other
        );
    }

    #[test]
    fn branch_pr_resolution_accepts_open_successor() {
        let json = br#"{"number":450,"headRefOid":"new",
            "url":"https://github.com/acme/amf/pull/450","state":"OPEN","headRefName":"feat-b"}"#;

        let pr = parse_open_resolved_pr(json).unwrap().unwrap();
        assert_eq!(pr.number, 450);
        assert_eq!(pr.head_sha, "new");
        assert_eq!(pr.owner, "acme");
        assert_eq!(pr.repo, "amf");
        assert_eq!(pr.head_ref, "feat-b");
    }

    #[test]
    fn branch_pr_resolution_rejects_closed_predecessor() {
        let json = br#"{"number":449,"headRefOid":"old",
            "url":"https://github.com/acme/amf/pull/449","state":"CLOSED"}"#;

        assert!(parse_open_resolved_pr(json).unwrap().is_none());
    }

    #[test]
    fn review_request_includes_commit_body_event_and_comments() {
        let comments = vec![
            PrReviewComment {
                path: "src/a.rs".into(),
                line: 12,
                side: "RIGHT",
                start_line: None,
                start_side: None,
                body: "this looks off".into(),
            },
            PrReviewComment {
                path: "src/a.rs".into(),
                line: 3,
                side: "LEFT",
                start_line: None,
                start_side: None,
                body: "why remove this?".into(),
            },
        ];
        let v = build_review_request_json("abc123", "Summary", "COMMENT", &comments);
        assert_eq!(v["commit_id"], "abc123");
        assert_eq!(v["body"], "Summary");
        assert_eq!(v["event"], "COMMENT");
        let arr = v["comments"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["path"], "src/a.rs");
        assert_eq!(arr[0]["line"], 12);
        assert_eq!(arr[0]["side"], "RIGHT");
        // A single-line comment omits the span keys entirely.
        assert!(arr[0].get("start_line").is_none());
        assert!(arr[0].get("start_side").is_none());
        assert_eq!(arr[1]["side"], "LEFT");
    }

    #[test]
    fn review_request_emits_start_line_for_multiline_comment() {
        let comments = vec![PrReviewComment {
            path: "src/a.rs".into(),
            line: 20,
            side: "RIGHT",
            start_line: Some(15),
            start_side: Some("RIGHT"),
            body: "this whole block".into(),
        }];
        let v = build_review_request_json("", "", "COMMENT", &comments);
        let arr = v["comments"].as_array().unwrap();
        assert_eq!(arr[0]["line"], 20);
        assert_eq!(arr[0]["start_line"], 15);
        assert_eq!(arr[0]["start_side"], "RIGHT");
    }

    #[test]
    fn summary_only_review_omits_empty_body_and_comments() {
        // No commit id, empty body, no inline comments: only `event` is present,
        // which GitHub accepts as a (bodyless) review action.
        let v = build_review_request_json("", "", "COMMENT", &[]);
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("commit_id"));
        assert!(!obj.contains_key("body"));
        assert!(!obj.contains_key("comments"));
        assert_eq!(v["event"], "COMMENT");
    }
}
