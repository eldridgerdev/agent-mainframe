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

/// A branch-scoped PR candidate used only while auto-resolving the current
/// feature. We intentionally ask GitHub for open and closed PRs together, then
/// choose an open one ourselves: bare `gh pr view` can otherwise restore an
/// older closed PR when the same head branch is reused for a later PR.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BranchPrCandidate {
    number: u32,
    #[serde(rename = "headRefOid")]
    head_sha: String,
    url: String,
    /// `OPEN`, `CLOSED`, or `MERGED`.
    #[serde(default)]
    state: String,
    /// ISO-8601, so lexical order is chronological order.
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
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

    /// Resolve the newest open PR for the current branch in `workdir`.
    ///
    /// Returns `NoPrForBranch` when the repo has a GitHub remote but no open PR
    /// for the branch (caller should offer the manual-number override). Returns
    /// an error for missing-remote / network / other failures.
    pub fn resolve_pr(workdir: &Path) -> Result<PrResolution> {
        let branch_output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(workdir)
            .output()
            .context("Failed to determine the current git branch.")?;
        if !branch_output.status.success() {
            let stderr = String::from_utf8_lossy(&branch_output.stderr);
            bail!(
                "Could not determine the current git branch: {}",
                stderr.trim()
            );
        }
        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();

        // Detached checkouts have no branch name for `--head`. Preserve the
        // old implicit lookup there; normal AMF features always take the
        // branch-scoped path below.
        if branch.is_empty() {
            return Self::resolve_pr_implicitly(workdir);
        }

        let output = Command::new("gh")
            .args([
                "pr",
                "list",
                "--head",
                &branch,
                "--state",
                "all",
                "--limit",
                "100",
                "--json",
                "number,headRefOid,url,state,updatedAt",
            ])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr list`.")?;

        if output.status.success() {
            return Ok(match parse_open_branch_pr(&output.stdout)? {
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
            PrViewError::Other => {
                bail!("`gh pr list` failed: {}", stderr.trim());
            }
        }
    }

    fn resolve_pr_implicitly(workdir: &Path) -> Result<PrResolution> {
        let output = Command::new("gh")
            .args(["pr", "view", "--json", "number,headRefOid,url"])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr view`.")?;

        if output.status.success() {
            return Ok(PrResolution::Found(parse_pr_json(&output.stdout)?));
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
                "number,headRefOid,url",
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

    /// Review-thread resolution state via GraphQL. REST can't report whether a
    /// thread is resolved, so this maps each member comment id to its thread.
    pub fn review_threads(
        workdir: &Path,
        owner: &str,
        repo: &str,
        number: u32,
    ) -> Result<Vec<ReviewThread>> {
        const QUERY: &str = "query($owner:String!,$repo:String!,$pr:Int!,$cursor:String){\
            repository(owner:$owner,name:$repo){pullRequest(number:$pr){\
            reviewThreads(first:100,after:$cursor){\
            pageInfo{hasNextPage endCursor}\
            nodes{id isResolved comments(first:100){nodes{databaseId}}}}}}}";

        let mut threads = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut cmd = Command::new("gh");
            cmd.args(["api", "graphql", "-f", &format!("query={QUERY}")])
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
    ) -> Result<()> {
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
        Ok(())
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

    let (owner, repo) = parse_owner_repo(&url)
        .with_context(|| format!("Could not parse owner/repo from PR url: {url}"))?;

    Ok(PrRef {
        number,
        head_sha,
        url,
        owner,
        repo,
    })
}

/// Pick the current PR from a branch-scoped `gh pr list --state all` result.
/// Closed predecessors are deliberately ignored; if GitHub somehow has more
/// than one open PR for the head, the most recently updated one wins (then the
/// larger PR number provides a deterministic tie-breaker).
fn parse_open_branch_pr(stdout: &[u8]) -> Result<Option<PrRef>> {
    let candidates: Vec<BranchPrCandidate> = serde_json::from_slice(stdout)
        .context("Failed to parse branch PR list from `gh pr list` JSON output.")?;
    let Some(candidate) = candidates
        .into_iter()
        .filter(|candidate| candidate.state.eq_ignore_ascii_case("open"))
        .max_by(|a, b| {
            a.updated_at
                .cmp(&b.updated_at)
                .then_with(|| a.number.cmp(&b.number))
        })
    else {
        return Ok(None);
    };
    let (owner, repo) = parse_owner_repo(&candidate.url)
        .with_context(|| format!("Could not parse owner/repo from PR url: {}", candidate.url))?;
    Ok(Some(PrRef {
        number: candidate.number,
        head_sha: candidate.head_sha,
        url: candidate.url,
        owner,
        repo,
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
        let json =
            br#"{"number":321,"headRefOid":"abc123","url":"https://github.com/o/r/pull/321"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.number, 321);
        assert_eq!(pr.head_sha, "abc123");
        assert_eq!(pr.owner, "o");
        assert_eq!(pr.repo, "r");
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
    fn branch_pr_resolution_prefers_open_successor_over_closed_predecessor() {
        let json = br#"[
            {"number":449,"headRefOid":"old","url":"https://github.com/acme/amf/pull/449",
             "state":"MERGED","updatedAt":"2026-07-12T12:00:00Z"},
            {"number":450,"headRefOid":"new","url":"https://github.com/acme/amf/pull/450",
             "state":"OPEN","updatedAt":"2026-07-13T12:00:00Z"}
        ]"#;

        let pr = parse_open_branch_pr(json).unwrap().unwrap();
        assert_eq!(pr.number, 450);
        assert_eq!(pr.head_sha, "new");
        assert_eq!(pr.owner, "acme");
        assert_eq!(pr.repo, "amf");
    }

    #[test]
    fn branch_pr_resolution_reports_no_pr_when_only_closed_history_exists() {
        let json = br#"[
            {"number":449,"headRefOid":"old","url":"https://github.com/acme/amf/pull/449",
             "state":"CLOSED","updatedAt":"2026-07-12T12:00:00Z"}
        ]"#;

        assert!(parse_open_branch_pr(json).unwrap().is_none());
    }

    #[test]
    fn branch_pr_resolution_uses_newest_open_candidate() {
        let json = br#"[
            {"number":450,"headRefOid":"first","url":"https://github.com/acme/amf/pull/450",
             "state":"OPEN","updatedAt":"2026-07-12T12:00:00Z"},
            {"number":451,"headRefOid":"latest","url":"https://github.com/acme/amf/pull/451",
             "state":"OPEN","updatedAt":"2026-07-13T12:00:00Z"}
        ]"#;

        assert_eq!(parse_open_branch_pr(json).unwrap().unwrap().number, 451);
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
