use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    app::App,
    app::pr_review::github_access::TriageGithub,
    github::{PrListEntry, PrMeta, PrRef, PrResolution, ReviewThread},
    gui_contract::GuiHandle,
};

/// The GitHub reads one step needs, named under the GUI lock and performed by
/// [`Self::run`] without it.
pub struct PrTriageReads {
    pub(super) github: Option<Arc<dyn TriageGithub>>,
    pub(super) workdir: PathBuf,
    pub(super) plan: ReadPlan,
}

#[derive(Default)]
pub(super) struct ReadPlan {
    pub(super) branch_pr: bool,
    pub(super) current_user: bool,
    pub(super) list: Option<bool>,
    pub(super) pr: Option<u32>,
    pub(super) meta: Option<u32>,
    pub(super) threads: Option<PrRef>,
}

impl PrTriageReads {
    pub(super) fn none() -> Self {
        Self {
            github: None,
            workdir: PathBuf::new(),
            plan: ReadPlan::default(),
        }
    }

    pub(super) fn new(app: &App, workdir: PathBuf, plan: ReadPlan) -> Self {
        Self {
            github: Some(app.pr_review_work.github()),
            workdir,
            plan,
        }
    }

    /// Perform the planned reads. Blocking: call it without the GUI lock.
    pub fn run(self) -> PrTriagePrefetch {
        let Some(github) = self.github else {
            return PrTriagePrefetch::default();
        };
        let (workdir, plan) = (&self.workdir, self.plan);
        PrTriagePrefetch {
            github: None,
            branch_pr: plan.branch_pr.then(|| github.resolve_pr(workdir)),
            current_user: plan.current_user.then(|| github.current_user(workdir)),
            list: plan.list.map(|c| (c, github.list_prs(workdir, c))),
            pr: plan.pr.map(|n| (n, github.fetch_pr_by_number(workdir, n))),
            meta: plan.meta.map(|n| (n, github.pr_meta(workdir, n))),
            threads: plan
                .threads
                .map(|pr| (pr.number, github.review_threads(workdir, &pr))),
            workdir: Some(self.workdir),
        }
    }
}

/// The results of [`PrTriageReads::run`]. A step takes each result it
/// planned; anything it reads that was not prefetched (or was read for
/// another checkout or pull request) is read live, so a plan that misses a
/// read costs a blocking call, never a stale answer.
#[derive(Default)]
pub struct PrTriagePrefetch {
    github: Option<Arc<dyn TriageGithub>>,
    workdir: Option<PathBuf>,
    branch_pr: Option<anyhow::Result<PrResolution>>,
    current_user: Option<anyhow::Result<String>>,
    list: Option<(bool, anyhow::Result<Vec<PrListEntry>>)>,
    pr: Option<(u32, anyhow::Result<PrRef>)>,
    meta: Option<(u32, anyhow::Result<PrMeta>)>,
    threads: Option<(u32, anyhow::Result<Vec<ReviewThread>>)>,
}

fn take_for<K: PartialEq, T>(slot: &mut Option<(K, T)>, key: K) -> Option<T> {
    slot.take().filter(|(k, _)| *k == key).map(|(_, v)| v)
}

impl PrTriagePrefetch {
    /// Attach the App's GitHub for live reads; every `*_prefetched` entry
    /// point calls this first.
    pub(super) fn attach(&mut self, gui: &mut GuiHandle) {
        self.github = Some(gui.app_for_workflow().pr_review_work.github());
    }

    fn live(&self) -> &dyn TriageGithub {
        self.github
            .as_deref()
            .expect("prefetch attached to the App")
    }

    fn fresh(&self, workdir: &Path) -> bool {
        self.workdir.as_deref() == Some(workdir)
    }

    pub(super) fn resolve_pr(&mut self, workdir: &Path) -> anyhow::Result<PrResolution> {
        match self.branch_pr.take().filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().resolve_pr(workdir),
        }
    }

    pub(super) fn current_user(&mut self, workdir: &Path) -> anyhow::Result<String> {
        match self.current_user.take().filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().current_user(workdir),
        }
    }

    /// The prefetched list only: `None` when it was not read.
    pub(super) fn prefetched_list(
        &mut self,
        workdir: &Path,
        include_closed: bool,
    ) -> Option<anyhow::Result<Vec<PrListEntry>>> {
        take_for(&mut self.list, include_closed).filter(|_| self.fresh(workdir))
    }

    pub(super) fn list_prs(
        &mut self,
        workdir: &Path,
        include_closed: bool,
    ) -> anyhow::Result<Vec<PrListEntry>> {
        match self.prefetched_list(workdir, include_closed) {
            Some(result) => result,
            None => self.live().list_prs(workdir, include_closed),
        }
    }

    pub(super) fn fetch_pr_by_number(
        &mut self,
        workdir: &Path,
        number: u32,
    ) -> anyhow::Result<PrRef> {
        match take_for(&mut self.pr, number).filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().fetch_pr_by_number(workdir, number),
        }
    }

    pub(super) fn pr_meta(&mut self, workdir: &Path, number: u32) -> anyhow::Result<PrMeta> {
        match take_for(&mut self.meta, number).filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().pr_meta(workdir, number),
        }
    }

    pub(super) fn review_threads(
        &mut self,
        workdir: &Path,
        pr: &PrRef,
    ) -> anyhow::Result<Vec<ReviewThread>> {
        match take_for(&mut self.threads, pr.number).filter(|_| self.fresh(workdir)) {
            Some(result) => result,
            None => self.live().review_threads(workdir, pr),
        }
    }
}
