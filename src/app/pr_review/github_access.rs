//! The GitHub reads and writes PR Triage's shared engines make, behind one
//! boundary. Production always uses [`GhTriageGithub`] (the existing `gh`
//! calls, unchanged); tests and the desktop adapter's regressions swap in a
//! fixture so no test reaches GitHub, and every write is countable.

use std::path::Path;

use anyhow::Result;

use super::PrReview;
use crate::github::{
    GhCli, GithubTransport, PrListEntry, PrMeta, PrRef, PrResolution, ReviewThread,
};

pub(crate) trait TriageGithub: Send + Sync {
    fn current_user(&self, workdir: &Path) -> Result<String>;
    fn list_prs(&self, workdir: &Path, include_closed: bool) -> Result<Vec<PrListEntry>>;
    fn resolve_pr(&self, workdir: &Path) -> Result<PrResolution>;
    fn fetch_pr_by_number(&self, workdir: &Path, number: u32) -> Result<PrRef>;
    /// Every comment source for `pr`, normalized ([`super::fetch_and_normalize`]).
    fn fetch_review(&self, workdir: &Path, pr: PrRef) -> Result<PrReview>;
    fn pr_meta(&self, workdir: &Path, number: u32) -> Result<PrMeta>;
    fn review_threads(&self, workdir: &Path, pr: &PrRef) -> Result<Vec<ReviewThread>>;
    /// GitHub write: reply into an inline review thread.
    fn reply_to_review_comment(
        &self,
        workdir: &Path,
        pr: &PrRef,
        root_comment_id: u64,
        body: &str,
    ) -> Result<()>;
    /// GitHub write: a new conversation comment on the PR.
    fn post_issue_comment(&self, workdir: &Path, pr: &PrRef, body: &str) -> Result<()>;
    /// GitHub write: resolve or reopen a review thread; returns its new state.
    fn set_thread_resolved(&self, workdir: &Path, thread_id: &str, resolved: bool) -> Result<bool>;
}

/// The production adapter: the same `gh` invocations PR Triage always made.
pub(crate) struct GhTriageGithub;

impl TriageGithub for GhTriageGithub {
    fn current_user(&self, workdir: &Path) -> Result<String> {
        GithubTransport::current_user(&GhCli, workdir)
    }

    fn list_prs(&self, workdir: &Path, include_closed: bool) -> Result<Vec<PrListEntry>> {
        GithubTransport::list_prs(&GhCli, workdir, include_closed)
    }

    fn resolve_pr(&self, workdir: &Path) -> Result<PrResolution> {
        GhCli::check_available()?;
        GhCli::check_auth()?;
        GhCli::resolve_pr(workdir)
    }

    fn fetch_pr_by_number(&self, workdir: &Path, number: u32) -> Result<PrRef> {
        GhCli::fetch_pr_by_number(workdir, number)
    }

    fn fetch_review(&self, workdir: &Path, pr: PrRef) -> Result<PrReview> {
        super::fetch_and_normalize(workdir, pr)
    }

    fn pr_meta(&self, workdir: &Path, number: u32) -> Result<PrMeta> {
        GhCli::pr_meta(workdir, number)
    }

    fn review_threads(&self, workdir: &Path, pr: &PrRef) -> Result<Vec<ReviewThread>> {
        GhCli::review_threads(workdir, &pr.owner, &pr.repo, pr.number)
    }

    fn reply_to_review_comment(
        &self,
        workdir: &Path,
        pr: &PrRef,
        root_comment_id: u64,
        body: &str,
    ) -> Result<()> {
        GhCli::reply_to_review_comment(
            workdir,
            &pr.owner,
            &pr.repo,
            pr.number,
            root_comment_id,
            body,
        )
    }

    fn post_issue_comment(&self, workdir: &Path, pr: &PrRef, body: &str) -> Result<()> {
        GhCli::post_issue_comment(workdir, &pr.owner, &pr.repo, pr.number, body)
    }

    fn set_thread_resolved(&self, workdir: &Path, thread_id: &str, resolved: bool) -> Result<bool> {
        GhCli::set_thread_resolved(workdir, thread_id, resolved)
    }
}

/// Blocking read-only investigation run. A plain `fn` so tests can stand in
/// for a paid harness, matching the other PR worker seams.
pub(crate) type InvestigationRunner = fn(&crate::project::AgentKind, &Path, &str) -> Result<String>;

/// An offline GitHub for tests: canned reads, recorded writes.
#[cfg(test)]
pub(crate) mod fake {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use anyhow::{Result, bail};

    use super::TriageGithub;
    use crate::app::pr_review::PrReview;
    use crate::github::{PrListEntry, PrMeta, PrRef, PrResolution, ReviewThread};

    #[derive(Default)]
    pub(crate) struct FakeState {
        pub(crate) prs: Vec<PrListEntry>,
        pub(crate) branch_pr: Option<u32>,
        pub(crate) head_sha: String,
        pub(crate) review: Option<PrReview>,
        pub(crate) meta: PrMeta,
        /// Thread id → resolved.
        pub(crate) threads: Vec<(String, bool, Vec<u64>)>,
        pub(crate) fail_writes: bool,
        pub(crate) fail_fetch: Option<String>,
        /// Fail every read except the comment fetch, as a hung or offline
        /// `gh` would.
        pub(crate) fail_reads: bool,
        /// Every write, in order, as `kind: detail`.
        pub(crate) writes: Vec<String>,
        pub(crate) fetches: usize,
    }

    #[derive(Clone, Default)]
    pub(crate) struct FakeGithub(pub(crate) Arc<Mutex<FakeState>>);

    impl FakeGithub {
        pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.0.lock().unwrap()
        }

        pub(crate) fn pr_ref(&self, number: u32) -> PrRef {
            PrRef {
                number,
                head_sha: self.state().head_sha.clone(),
                url: format!("https://github.com/demo/repo/pull/{number}"),
                owner: "demo".into(),
                repo: "repo".into(),
                head_ref: "feature".into(),
            }
        }

        fn read(&self) -> Result<std::sync::MutexGuard<'_, FakeState>> {
            let state = self.state();
            if state.fail_reads {
                bail!("fixture GitHub is unreachable");
            }
            Ok(state)
        }

        fn write(&self, entry: String) -> Result<()> {
            let mut state = self.state();
            if state.fail_writes {
                bail!("fixture GitHub refused the write");
            }
            state.writes.push(entry);
            Ok(())
        }
    }

    impl TriageGithub for FakeGithub {
        fn current_user(&self, _: &Path) -> Result<String> {
            drop(self.read()?);
            Ok("reviewer".into())
        }

        fn list_prs(&self, _: &Path, include_closed: bool) -> Result<Vec<PrListEntry>> {
            Ok(self
                .read()?
                .prs
                .iter()
                .filter(|p| include_closed || p.state == "OPEN")
                .cloned()
                .collect())
        }

        fn resolve_pr(&self, _: &Path) -> Result<PrResolution> {
            let number = self.read()?.branch_pr;
            Ok(match number {
                Some(n) => PrResolution::Found(self.pr_ref(n)),
                None => PrResolution::NoPrForBranch,
            })
        }

        fn fetch_pr_by_number(&self, _: &Path, number: u32) -> Result<PrRef> {
            drop(self.read()?);
            Ok(self.pr_ref(number))
        }

        fn fetch_review(&self, _: &Path, pr: PrRef) -> Result<PrReview> {
            let mut state = self.state();
            state.fetches += 1;
            if let Some(error) = &state.fail_fetch {
                bail!("{error}");
            }
            let mut review = state.review.clone().expect("fixture review");
            review.pr = pr;
            for comment in &mut review.comments {
                if let Some((id, resolved, _)) = state
                    .threads
                    .iter()
                    .find(|(_, _, ids)| ids.contains(&comment.id))
                {
                    comment.thread_id = Some(id.clone());
                    comment.is_resolved = *resolved;
                }
            }
            Ok(review)
        }

        fn pr_meta(&self, _: &Path, _: u32) -> Result<PrMeta> {
            Ok(self.read()?.meta.clone())
        }

        fn review_threads(&self, _: &Path, _: &PrRef) -> Result<Vec<ReviewThread>> {
            Ok(self
                .read()?
                .threads
                .iter()
                .map(|(id, resolved, ids)| ReviewThread {
                    id: id.clone(),
                    is_resolved: *resolved,
                    comment_ids: ids.clone(),
                })
                .collect())
        }

        fn reply_to_review_comment(
            &self,
            _: &Path,
            _: &PrRef,
            root_comment_id: u64,
            body: &str,
        ) -> Result<()> {
            self.write(format!("reply {root_comment_id}: {body}"))
        }

        fn post_issue_comment(&self, _: &Path, pr: &PrRef, body: &str) -> Result<()> {
            self.write(format!("comment #{}: {body}", pr.number))
        }

        fn set_thread_resolved(&self, _: &Path, thread_id: &str, resolved: bool) -> Result<bool> {
            self.write(format!("resolve {thread_id}: {resolved}"))?;
            let mut state = self.state();
            if let Some(thread) = state.threads.iter_mut().find(|t| t.0 == thread_id) {
                thread.1 = resolved;
            }
            Ok(resolved)
        }
    }
}
