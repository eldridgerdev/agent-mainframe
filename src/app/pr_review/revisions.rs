//! Materialize a pull request's revisions for a manual review, object-only.
//!
//! The PR head (`pull/<n>/head`, which the base repository carries even for a
//! fork PR) and its base branch are fetched into private refs under
//! `refs/amf/review/<n>/`. Nothing else changes: not HEAD, not the index, not
//! the working tree, not the stash, not any `refs/remotes/*` tracking ref. The
//! review then reads everything by OID, so the refs exist only to keep the
//! objects reachable while the review is open.
//!
//! Every function here blocks on `git` and runs from a worker thread.

use std::fmt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::github::ReviewablePr;

/// Namespace for every PR review's refs. Shared across the repository's
/// worktrees, like every ref outside `refs/worktree/`.
const REVIEW_REF_PREFIX: &str = "refs/amf/review/";

/// The private ref holding one side (`head` / `base`) of PR `number`.
pub(crate) fn review_ref(number: u32, side: &str) -> String {
    format!("{REVIEW_REF_PREFIX}{number}/{side}")
}

/// The revisions a PR review is pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializedRevisions {
    pub(crate) number: u32,
    pub(crate) base_oid: String,
    pub(crate) head_oid: String,
    /// `git merge-base base head`. The review diffs `merge_base..head`, which
    /// is GitHub's three-dot "Files changed" view.
    pub(crate) merge_base_oid: String,
}

#[derive(Debug)]
pub(crate) enum MaterializeError {
    /// The fetched head isn't the one `gh` reported: the PR moved between
    /// the two calls. (The base isn't checked: `gh` doesn't report its OID,
    /// and GitHub diffs against the base branch's current tip anyway.)
    Moved {
        expected_head: String,
        fetched_head: String,
    },
    Git(anyhow::Error),
}

impl fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Moved {
                expected_head,
                fetched_head,
            } => write!(
                f,
                "the PR moved while loading (head {} → {})",
                short(expected_head),
                short(fetched_head)
            ),
            Self::Git(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for MaterializeError {}

fn short(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}

/// Fetch `pr`'s head and base from `fetch_url` into its private refs, check
/// the head against the OID `gh` reported, and compute the merge-base
/// against the base branch tip as fetched.
pub(crate) fn fetch_pr_revisions(
    workdir: &Path,
    fetch_url: &str,
    pr: &ReviewablePr,
) -> Result<MaterializedRevisions, MaterializeError> {
    let head_ref = review_ref(pr.number, "head");
    let base_ref = review_ref(pr.number, "base");
    git(
        workdir,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            // No FETCH_HEAD, no submodule fetches: the fetch writes the two
            // refs below and the objects behind them, nothing else.
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            fetch_url,
            &format!("+refs/pull/{}/head:{head_ref}", pr.number),
            &format!("+refs/heads/{}:{base_ref}", pr.base_ref),
        ],
    )
    .with_context(|| format!("Could not fetch PR #{}", pr.number))
    .map_err(MaterializeError::Git)?;

    let fetched_head = rev_parse(workdir, &head_ref).map_err(MaterializeError::Git)?;
    let fetched_base = rev_parse(workdir, &base_ref).map_err(MaterializeError::Git)?;
    if fetched_head != pr.head_oid {
        return Err(MaterializeError::Moved {
            expected_head: pr.head_oid.clone(),
            fetched_head,
        });
    }

    let merge_base_oid = git(workdir, &["merge-base", &fetched_base, &fetched_head])
        .with_context(|| {
            format!(
                "PR #{} has no common history with its base branch `{}`",
                pr.number, pr.base_ref
            )
        })
        .map_err(MaterializeError::Git)?;
    Ok(MaterializedRevisions {
        number: pr.number,
        base_oid: fetched_base,
        head_oid: fetched_head,
        merge_base_oid,
    })
}

/// [`fetch_pr_revisions`], refreshing `pr` from GitHub and retrying once if
/// it moved mid-load. `refresh` returns the PR's current state (in
/// production, [`crate::github::GhCli::pr_revisions`]). Leaves no refs behind
/// on failure.
pub(crate) fn materialize(
    workdir: &Path,
    fetch_url: &str,
    pr: &ReviewablePr,
    refresh: impl FnOnce() -> Result<ReviewablePr>,
) -> Result<MaterializedRevisions> {
    let result = match fetch_pr_revisions(workdir, fetch_url, pr) {
        Err(MaterializeError::Moved { .. }) => match refresh() {
            Ok(current) => {
                fetch_pr_revisions(workdir, fetch_url, &current).map_err(|err| match err {
                    MaterializeError::Moved { .. } => anyhow::anyhow!(
                        "PR #{} kept changing while it loaded ({err}). Try again in a moment.",
                        pr.number
                    ),
                    MaterializeError::Git(err) => err,
                })
            }
            Err(err) => Err(err.context(format!(
                "PR #{} changed while loading and could not be re-read",
                pr.number
            ))),
        },
        other => other.map_err(|err| match err {
            MaterializeError::Git(err) => err,
            moved => anyhow::anyhow!("{moved}"),
        }),
    };
    if result.is_err() {
        let _ = remove_review_refs(workdir, pr.number);
    }
    result
}

/// Delete PR `number`'s review refs, when a review is closed or posted. The
/// objects stay until an ordinary `git gc` finds them unreachable.
pub(crate) fn remove_review_refs(workdir: &Path, number: u32) -> Result<()> {
    for side in ["head", "base"] {
        let reference = review_ref(number, side);
        if rev_parse(workdir, &reference).is_ok() {
            git(workdir, &["update-ref", "-d", &reference])?;
        }
    }
    Ok(())
}

/// Delete every PR review ref in the repository, returning how many went.
/// For clearing refs a crash left behind. Safe even while a review is open
/// in another AMF process, because reviews read by OID and the objects
/// outlive their refs until `git gc`'s expiry.
pub(crate) fn prune_review_refs(workdir: &Path) -> Result<usize> {
    let listed = git(
        workdir,
        &["for-each-ref", "--format=%(refname)", REVIEW_REF_PREFIX],
    )?;
    let mut removed = 0;
    for reference in listed.lines().filter(|l| !l.is_empty()) {
        git(workdir, &["update-ref", "-d", reference])?;
        removed += 1;
    }
    Ok(removed)
}

fn rev_parse(workdir: &Path, reference: &str) -> Result<String> {
    git(
        workdir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
    )
}

/// Run `git`, returning trimmed stdout.
///
/// Never prompts (see [`crate::github::without_git_prompts`]): these run on a
/// worker thread under the TUI. A missing-credentials failure is reported with
/// the fix rather than git's raw stderr.
fn git(workdir: &Path, args: &[&str]) -> Result<String> {
    let output = crate::github::without_git_prompts(&mut Command::new("git"))
        .args(args)
        .current_dir(workdir)
        .output()
        .with_context(|| format!("Failed to run `git {}`", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if let Some(hint) = crate::github::git_credentials_hint(&stderr) {
            bail!("{hint}");
        }
        bail!("`git {}` failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn run(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn commit_file(dir: &Path, path: &str, content: &str, message: &str) -> String {
        std::fs::write(dir.join(path), content).unwrap();
        run(dir, &["add", path]);
        run(dir, &["commit", "--quiet", "-m", message]);
        run(dir, &["rev-parse", "HEAD"]).trim().to_string()
    }

    fn identity(dir: &Path) {
        run(dir, &["config", "user.name", "AMF Test"]);
        run(dir, &["config", "user.email", "amf@example.com"]);
    }

    /// An upstream repository with PR #7 whose base branch has moved on since
    /// the PR branched, plus a local clone with a feature branch carrying
    /// staged, unstaged, and untracked changes and a stash entry.
    struct Fixture {
        _root: TempDir,
        upstream: PathBuf,
        local: PathBuf,
        fork_point: String,
        pr_head: String,
        base_tip: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let upstream = root.path().join("upstream");
            std::fs::create_dir(&upstream).unwrap();
            run(&upstream, &["init", "--quiet", "--initial-branch=main"]);
            identity(&upstream);
            let fork_point = commit_file(&upstream, "lib.rs", "fn a() {}\n", "initial");

            run(&upstream, &["checkout", "--quiet", "-b", "contrib"]);
            let pr_head = commit_file(&upstream, "lib.rs", "fn a() {}\nfn b() {}\n", "add b");
            // What GitHub does for every PR, fork or not.
            run(&upstream, &["update-ref", "refs/pull/7/head", &pr_head]);
            run(&upstream, &["checkout", "--quiet", "main"]);
            run(&upstream, &["branch", "--quiet", "-D", "contrib"]);
            let base_tip = commit_file(&upstream, "other.rs", "// later\n", "base moves on");

            let local = root.path().join("local");
            run(
                root.path(),
                &["clone", "--quiet", upstream.to_str().unwrap(), "local"],
            );
            identity(&local);
            run(&local, &["checkout", "--quiet", "-b", "my-feature"]);
            commit_file(&local, "mine.rs", "one\n", "feature work");
            // A stash entry, then fresh dirt of every kind on top of it.
            std::fs::write(local.join("mine.rs"), "stashed\n").unwrap();
            run(&local, &["stash", "push", "--quiet", "-m", "keep me"]);
            std::fs::write(local.join("mine.rs"), "unstaged edit\n").unwrap();
            std::fs::write(local.join("lib.rs"), "staged edit\n").unwrap();
            run(&local, &["add", "lib.rs"]);
            std::fs::write(local.join("scratch.txt"), "untracked\n").unwrap();

            Self {
                _root: root,
                upstream,
                local,
                fork_point,
                pr_head,
                base_tip,
            }
        }

        fn url(&self) -> String {
            self.upstream.to_str().unwrap().to_string()
        }

        fn pr(&self) -> ReviewablePr {
            ReviewablePr {
                number: 7,
                title: "Add b".to_string(),
                author: "teammate".to_string(),
                is_draft: false,
                updated_at: String::new(),
                base_ref: "main".to_string(),
                // As `gh` lists it: no base OID (see `ReviewablePr::base_oid`).
                base_oid: String::new(),
                head_ref: "contrib".to_string(),
                head_oid: self.pr_head.clone(),
                is_cross_repository: true,
                head_owner: "teammate".to_string(),
            }
        }
    }

    /// Everything a review must leave byte-identical in the user's checkout.
    fn checkout_state(dir: &Path) -> Vec<String> {
        vec![
            run(dir, &["symbolic-ref", "HEAD"]),
            run(dir, &["rev-parse", "HEAD"]),
            run(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
            run(dir, &["ls-files", "--stage"]),
            run(dir, &["diff", "--cached"]),
            run(dir, &["diff"]),
            run(dir, &["stash", "list", "--format=%H %gs"]),
            run(
                dir,
                &[
                    "for-each-ref",
                    "--format=%(refname) %(objectname)",
                    "refs/heads",
                    "refs/remotes",
                ],
            ),
            std::fs::read_to_string(dir.join("scratch.txt")).unwrap(),
            std::fs::read_to_string(dir.join("mine.rs")).unwrap(),
        ]
    }

    #[test]
    fn materializing_a_pr_leaves_the_checkout_byte_identical() {
        let fx = Fixture::new();
        let before = checkout_state(&fx.local);

        let revisions = materialize(&fx.local, &fx.url(), &fx.pr(), || {
            panic!("nothing moved, so no refresh")
        })
        .unwrap();

        assert_eq!(checkout_state(&fx.local), before);
        assert_eq!(revisions.head_oid, fx.pr_head);
        assert_eq!(revisions.base_oid, fx.base_tip);
        // The fork point, not the base tip: GitHub's three-dot view.
        assert_eq!(revisions.merge_base_oid, fx.fork_point);
        assert_eq!(
            rev_parse(&fx.local, &review_ref(7, "head")).unwrap(),
            fx.pr_head
        );
        assert_eq!(
            rev_parse(&fx.local, &review_ref(7, "base")).unwrap(),
            fx.base_tip
        );
    }

    #[test]
    fn removing_review_refs_leaves_the_checkout_byte_identical() {
        let fx = Fixture::new();
        let before = checkout_state(&fx.local);
        materialize(&fx.local, &fx.url(), &fx.pr(), || unreachable!()).unwrap();

        remove_review_refs(&fx.local, 7).unwrap();

        assert!(rev_parse(&fx.local, &review_ref(7, "head")).is_err());
        assert!(rev_parse(&fx.local, &review_ref(7, "base")).is_err());
        assert_eq!(checkout_state(&fx.local), before);
        // Idempotent: closing twice is not an error.
        remove_review_refs(&fx.local, 7).unwrap();
    }

    #[test]
    fn a_pr_that_moved_mid_load_is_refreshed_and_retried_once() {
        let fx = Fixture::new();
        let mut stale = fx.pr();
        stale.head_oid = fx.fork_point.clone(); // what `gh` said a moment ago
        let current = fx.pr();

        let revisions = materialize(&fx.local, &fx.url(), &stale, || Ok(current)).unwrap();

        assert_eq!(revisions.head_oid, fx.pr_head);
    }

    #[test]
    fn a_pr_that_keeps_moving_fails_and_leaves_no_refs() {
        let fx = Fixture::new();
        let mut stale = fx.pr();
        stale.head_oid = fx.fork_point.clone();
        let still_stale = stale.clone();

        let err = materialize(&fx.local, &fx.url(), &stale, || Ok(still_stale)).unwrap_err();

        assert!(err.to_string().contains("kept changing"), "{err:#}");
        assert!(rev_parse(&fx.local, &review_ref(7, "head")).is_err());
        assert!(rev_parse(&fx.local, &review_ref(7, "base")).is_err());
    }

    #[test]
    fn a_missing_pr_ref_is_a_git_error_not_a_move() {
        let fx = Fixture::new();
        let mut missing = fx.pr();
        missing.number = 8;
        let err = fetch_pr_revisions(&fx.local, &fx.url(), &missing).unwrap_err();
        assert!(matches!(err, MaterializeError::Git(_)), "{err}");
    }

    #[test]
    fn pruning_removes_every_review_ref_and_nothing_else() {
        let fx = Fixture::new();
        materialize(&fx.local, &fx.url(), &fx.pr(), || unreachable!()).unwrap();
        // Another AMF feature's private ref, outside the review namespace.
        run(
            &fx.local,
            &["update-ref", "refs/amf/pr-7-123-head", &fx.pr_head],
        );

        assert_eq!(prune_review_refs(&fx.local).unwrap(), 2);

        assert_eq!(prune_review_refs(&fx.local).unwrap(), 0);
        assert!(rev_parse(&fx.local, "refs/amf/pr-7-123-head").is_ok());
    }
}
