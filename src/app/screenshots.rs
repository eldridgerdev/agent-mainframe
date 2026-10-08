//! App-owned evidence watcher and producer registration. Reading workers use snapshots.
use super::App;
use crate::screenshot_evidence::EvidenceOwner;
use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(crate) struct EvidenceWork {
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    dirty: Arc<AtomicBool>,
    pub(crate) remote: Option<RemoteScope>,
    pub(crate) closed_requests: std::collections::VecDeque<String>,
}

impl EvidenceWork {
    pub(crate) fn watch(&mut self, owners: &[EvidenceOwner]) {
        if self.watcher.is_none() {
            let dirty = self.dirty.clone();
            self.watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if !matches!(event,Ok(ref e) if matches!(e.kind,notify::EventKind::Access(_))) {
                        dirty.store(true, Ordering::Release);
                    }
                })
                .ok();
        }
        let wanted: HashSet<PathBuf> = owners
            .iter()
            .filter_map(|owner| {
                crate::screenshot_evidence::safe_path(&owner.directory()).ok()?;
                let mut path = owner.directory();
                while !path.is_dir() {
                    path = path.parent()?.to_path_buf();
                }
                Some(path)
            })
            .collect();
        if let Some(watcher) = &mut self.watcher {
            for path in self.watched.difference(&wanted) {
                let _ = watcher.unwatch(path);
            }
            self.watched.retain(|p| wanted.contains(p));
            for path in wanted
                .difference(&self.watched)
                .cloned()
                .collect::<Vec<_>>()
            {
                if watcher.watch(&path, RecursiveMode::Recursive).is_ok() {
                    self.watched.insert(path);
                }
            }
        }
    }
    pub(crate) fn changed(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }
}

impl App {
    pub(crate) fn screenshot_owner(
        &self,
        project_index: usize,
        feature_index: usize,
        session_id: &str,
    ) -> Result<Option<EvidenceOwner>> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        let project = &self.store.projects[project_index];
        let feature = &project.features[feature_index];
        let session = feature
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("Screenshot producing session disappeared"))?;
        let owner = EvidenceOwner {
            version: 1,
            scope_id: uuid::Uuid::new_v4().to_string(),
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
            session_id: session.id.clone(),
            project_name: project.name.clone(),
            feature_name: feature.name.clone(),
            session_label: session.label.clone(),
            workdir: feature.workdir.canonicalize()?,
            is_worktree: feature.is_worktree,
            created_at: chrono::Utc::now(),
        };
        db.register_evidence(owner).map(Some)
    }

    pub(crate) fn screenshot_launch_args(
        &self,
        session_id: &str,
        claude: bool,
        mut args: Vec<String>,
    ) -> Result<Vec<String>> {
        let mut locations = self.store.projects.iter().enumerate().flat_map(|(pi, p)| {
            p.features.iter().enumerate().filter_map(move |(fi, f)| {
                f.sessions
                    .iter()
                    .any(|s| s.id == session_id)
                    .then_some((pi, fi))
            })
        });
        let location = locations.next();
        anyhow::ensure!(
            locations.next().is_none(),
            "Ambiguous screenshot producing session"
        );
        if let Some((pi, fi)) = location
            && let Some(owner) = self.screenshot_owner(pi, fi, session_id)?
        {
            args = launch_args(&owner, claude, args)?;
        }
        Ok(args)
    }

    /// Call only after confirmed successful deletion. Missing root evidence is retained.
    pub(crate) fn screenshot_worktree_deleted(&mut self, workdir: &Path) -> Result<()> {
        if self
            .evidence_work
            .remote
            .as_ref()
            .is_some_and(|r| r.context.workdir == workdir)
            && let Some(remote) = self.evidence_work.remote.take()
        {
            remote.cancelled.store(true, Ordering::Release);
        }
        if let Some(db) = &self.db {
            for (owner, _) in db.evidence_scopes(false)? {
                if owner.is_worktree
                    && (owner.workdir == workdir
                        || workdir.canonicalize().is_ok_and(|p| p == owner.workdir))
                {
                    db.retire_evidence(&owner.scope_id)?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn launch_args(
    owner: &EvidenceOwner,
    claude: bool,
    mut args: Vec<String>,
) -> Result<Vec<String>> {
    let guidance = crate::screenshot_evidence::guidance(owner);
    if claude {
        args.extend(["--append-system-prompt".into(), guidance]);
    } else {
        args = crate::codex_config::with_screenshot_guidance(&owner.workdir, &guidance, args)?;
    }
    Ok(args)
}

pub(crate) struct RemoteScope {
    pub request_id: String,
    pub context: crate::screenshot_sources::PrContext,
    pub resources: std::collections::HashMap<String, crate::screenshot_sources::Resource>,
    pub cancelled: Arc<AtomicBool>,
}
