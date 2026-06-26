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
    #[serde(default)]
    pub diff_hunk: Option<String>,
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
/// `"RIGHT"` for the current file or `"LEFT"` for the base file, matching the
/// GitHub create-review API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrReviewComment {
    pub path: String,
    pub line: u32,
    pub side: &'static str,
    pub body: String,
}

impl GhCli {
    /// Check that a working `gh` binary is on PATH. Mirrors
    /// `TmuxManager::check_available` / `ClaudeLauncher::check_available`.
    pub fn check_available() -> Result<()> {
        let output = Command::new("gh")
            .arg("--version")
            .output()
            .context("GitHub CLI (`gh`) not found. Install it from https://cli.github.com to use PR review.")?;
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
            .args(["pr", "view", "--json", "number,headRefOid,url"])
            .current_dir(workdir)
            .output()
            .context("Failed to run `gh pr view`.")?;

        if output.status.success() {
            let pr = parse_pr_json(&output.stdout)?;
            return Ok(PrResolution::Found(pr));
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        match classify_pr_view_error(&stderr) {
            PrViewError::NoPr => Ok(PrResolution::NoPrForBranch),
            PrViewError::NoRemote => bail!(
                "No GitHub remote found for this repository. PR review needs a GitHub-hosted repo."
            ),
            PrViewError::Other => {
                bail!("`gh pr view` failed: {}", stderr.trim());
            }
        }
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

    /// Inline review comments for a PR (file/line-anchored), all pages.
    pub fn pr_review_comments(workdir: &Path, number: u32) -> Result<Vec<ReviewComment>> {
        fetch_paginated(workdir, &format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments"))
    }

    /// Top-level review summaries (Approve / Request-changes / Comment), all pages.
    pub fn pr_reviews(workdir: &Path, number: u32) -> Result<Vec<Review>> {
        fetch_paginated(workdir, &format!("repos/{{owner}}/{{repo}}/pulls/{number}/reviews"))
    }

    /// Conversation comments on the PR's issue timeline, all pages.
    pub fn issue_comments(workdir: &Path, number: u32) -> Result<Vec<IssueComment>> {
        fetch_paginated(workdir, &format!("repos/{{owner}}/{{repo}}/issues/{number}/comments"))
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
                bail!("`gh api graphql` (review threads) failed: {}", stderr.trim());
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
            bail!("`gh api graphql` (resolve thread) failed: {}", stderr.trim());
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
                json!({
                    "path": c.path,
                    "line": c.line,
                    "side": c.side,
                    "body": c.body,
                })
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
        let json = br#"{"number":321,"headRefOid":"abc123","url":"https://github.com/o/r/pull/321"}"#;
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
    fn review_request_includes_commit_body_event_and_comments() {
        let comments = vec![
            PrReviewComment {
                path: "src/a.rs".into(),
                line: 12,
                side: "RIGHT",
                body: "this looks off".into(),
            },
            PrReviewComment {
                path: "src/a.rs".into(),
                line: 3,
                side: "LEFT",
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
        assert_eq!(arr[1]["side"], "LEFT");
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
