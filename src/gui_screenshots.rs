//! Narrow GUI bridge for local evidence. Scan/decode outside the GuiHandle lock.
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
pub use crate::screenshot_evidence::{
    EvidenceIssue, EvidenceItem, EvidenceListing, EvidenceOwner, ImageData,
};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EvidenceSelection {
    pub feature: Option<FeatureTarget>,
    pub session_id: Option<String>,
}

pub struct EvidenceRead {
    owners: Vec<EvidenceOwner>,
}
impl EvidenceRead {
    pub fn run(self) -> EvidenceListing {
        crate::screenshot_evidence::scan(self.owners)
    }
}

pub fn plan(gui: &mut GuiHandle, selection: EvidenceSelection) -> GuiResult<EvidenceRead> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    if let Some(target) = &selection.feature {
        let (pi, fi) = app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
            .ok_or_else(|| GuiError::not_found("Feature was deleted; open retained screenshots"))?;
        let _ = (pi, fi);
    }
    let db = app
        .db
        .as_ref()
        .ok_or_else(|| GuiError::not_found("No evidence database attached"))?;
    // External worktree deletion invalidates owned scopes, never retained root evidence.
    for (owner, _) in db.evidence_scopes(false).map_err(GuiError::from)? {
        if owner.is_worktree && !owner.workdir.exists() {
            db.retire_evidence(&owner.scope_id)
                .map_err(GuiError::from)?;
        }
    }
    let owners: Vec<_> = db
        .evidence_scopes(false)
        .map_err(GuiError::from)?
        .into_iter()
        .map(|(o, _)| o)
        .filter(|o| {
            selection
                .feature
                .as_ref()
                .is_none_or(|f| f.project_id == o.project_id && f.feature_id == o.feature_id)
                && selection
                    .session_id
                    .as_ref()
                    .is_none_or(|s| s == &o.session_id)
        })
        .collect();
    app.evidence_work.watch(&owners);
    Ok(EvidenceRead { owners })
}

pub fn finish(gui: &mut GuiHandle, mut listing: EvidenceListing) -> GuiResult<EvidenceListing> {
    let db = gui.db()?;
    let mut active = std::collections::HashSet::new();
    for owner in &listing.owners {
        if owner.is_worktree && !owner.workdir.exists() {
            db.retire_evidence(&owner.scope_id)
                .map_err(GuiError::from)?;
            continue;
        }
        if db
            .evidence_scope_active(&owner.scope_id)
            .map_err(GuiError::from)?
        {
            active.insert(owner.scope_id.clone());
        }
    }
    listing.items.retain(|i| active.contains(&i.scope_id));
    listing.issues.retain(|i| active.contains(&i.scope_id));
    listing.owners.retain(|i| active.contains(&i.scope_id));
    Ok(listing)
}

pub fn changed(gui: &mut GuiHandle) -> bool {
    gui.app_for_workflow().evidence_work.changed()
}

pub struct ImageRead {
    owner: EvidenceOwner,
    image_id: String,
    hash: String,
    thumbnail: bool,
}
impl ImageRead {
    pub fn run(&self) -> GuiResult<ImageData> {
        let _permit = crate::screenshot_evidence::image_worker().map_err(GuiError::from)?;
        crate::screenshot_evidence::load_image(
            &self.owner,
            &self.image_id,
            &self.hash,
            self.thumbnail,
        )
        .map_err(GuiError::from)
    }
}
pub fn plan_image(
    gui: &GuiHandle,
    scope_id: String,
    image_id: String,
    hash: String,
    thumbnail: bool,
) -> GuiResult<ImageRead> {
    let owner = gui
        .db()?
        .evidence_scopes(false)
        .map_err(GuiError::from)?
        .into_iter()
        .find(|(o, _)| o.scope_id == scope_id)
        .map(|(o, _)| o)
        .ok_or_else(|| GuiError::conflict("Evidence was cleaned up; refresh screenshots"))?;
    Ok(ImageRead {
        owner,
        image_id,
        hash,
        thumbnail,
    })
}
pub fn finish_image(gui: &GuiHandle, read: &ImageRead) -> GuiResult<()> {
    if read.owner.is_worktree && !read.owner.workdir.exists() {
        gui.db()?
            .retire_evidence(&read.owner.scope_id)
            .map_err(GuiError::from)?;
    }
    if !gui
        .db()?
        .evidence_scope_active(&read.owner.scope_id)
        .map_err(GuiError::from)?
    {
        return Err(GuiError::conflict(
            "Evidence was cleaned up; refresh screenshots",
        ));
    }
    Ok(())
}

/// Includes retired scopes so a failed removal can be retried without live records.
pub fn cleanup_scopes(gui: &GuiHandle) -> GuiResult<Vec<(EvidenceOwner, bool)>> {
    gui.db()?.evidence_scopes(true).map_err(GuiError::from)
}

pub fn plan_cleanup(gui: &mut GuiHandle, scope_id: &str) -> GuiResult<CleanupRead> {
    let db = gui.db()?;
    let owner = db
        .evidence_scopes(true)
        .map_err(GuiError::from)?
        .into_iter()
        .find(|(o, _)| o.scope_id == scope_id)
        .map(|(o, _)| o)
        .ok_or_else(|| GuiError::not_found("Screenshot scope no longer exists"))?;
    db.retire_evidence(scope_id).map_err(GuiError::from)?;
    Ok(CleanupRead { owner })
}

pub use crate::screenshot_sources::{
    BrowserGallery, RemoteItem, RemoteListing, RunChoice, SourceIssue,
};
use crate::screenshot_sources::{EvidenceGithub, GithubEvidence, PrContext, Resource, Retrieved};
use std::sync::Arc;

pub struct RemoteRead {
    context: PrContext,
    request_id: String,
    selected_run: Option<u64>,
    run_page: u32,
    github: Arc<dyn EvidenceGithub>,
}
impl RemoteRead {
    pub fn run(self) -> GuiResult<RemoteResult> {
        let retrieved = crate::screenshot_sources::retrieve(
            &self.context,
            self.request_id,
            self.selected_run,
            self.run_page,
            self.github.as_ref(),
        )
        .map_err(GuiError::from)?;
        Ok(RemoteResult {
            context: self.context,
            retrieved,
        })
    }
}
pub struct RemoteResult {
    context: PrContext,
    retrieved: Retrieved,
}

pub fn plan_remote(
    gui: &mut GuiHandle,
    workflow_id: &str,
    selected_run: Option<u64>,
    run_page: u32,
    request_id: String,
) -> GuiResult<RemoteRead> {
    let context = crate::gui_pr_triage::screenshot_context(gui, workflow_id)?;
    if !crate::screenshot_evidence::valid_id(&request_id)
        || gui
            .app_for_workflow()
            .evidence_work
            .closed_requests
            .contains(&request_id)
    {
        return Err(GuiError::conflict("Screenshot request was closed"));
    }
    if let Some(previous) = &gui.app_for_workflow().evidence_work.remote {
        previous
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    gui.app_for_workflow().evidence_work.remote = Some(crate::app::screenshots::RemoteScope {
        request_id: request_id.clone(),
        context: context.clone(),
        resources: std::collections::HashMap::new(),
        cancelled: cancelled.clone(),
    });
    Ok(RemoteRead {
        context,
        request_id,
        selected_run,
        run_page,
        github: Arc::new(CancellableGithub {
            inner: Arc::new(GithubEvidence),
            cancelled,
        }),
    })
}
fn fresh_remote(gui: &mut GuiHandle, id: &str) -> GuiResult<PrContext> {
    let context = gui
        .app_for_workflow()
        .evidence_work
        .remote
        .as_ref()
        .filter(|s| s.request_id == id)
        .map(|s| s.context.clone())
        .ok_or_else(|| GuiError::conflict("Screenshot selection changed or closed"))?;
    let current = crate::gui_pr_triage::screenshot_context(gui, &context.workflow_id)?;
    if current.number != context.number
        || current.head_sha != context.head_sha
        || current.feature_id != context.feature_id
    {
        return Err(GuiError::conflict("PR screenshot selection changed"));
    }
    Ok(current)
}
pub fn finish_remote(gui: &mut GuiHandle, result: RemoteResult) -> GuiResult<RemoteListing> {
    let current = fresh_remote(gui, &result.retrieved.listing.request_id)?;
    if current.head_sha != result.context.head_sha || current.number != result.context.number {
        return Err(GuiError::conflict("PR screenshot context changed"));
    }
    gui.app_for_workflow()
        .evidence_work
        .remote
        .as_mut()
        .unwrap()
        .resources = result.retrieved.resources;
    Ok(result.retrieved.listing)
}

pub struct RemoteImageRead {
    context: PrContext,
    resource: Resource,
    request_id: String,
    thumbnail: bool,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}
impl RemoteImageRead {
    pub fn run(&self) -> GuiResult<ImageData> {
        let _permit = crate::screenshot_evidence::image_worker().map_err(GuiError::from)?;
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(GuiError::conflict("Screenshot request was closed"));
        }
        crate::screenshot_sources::resource_image(
            &self.context,
            &self.resource,
            self.thumbnail,
            &GithubEvidence,
        )
        .map_err(GuiError::from)
    }
}
pub fn plan_remote_image(
    gui: &mut GuiHandle,
    request_id: String,
    key: &str,
    thumbnail: bool,
) -> GuiResult<RemoteImageRead> {
    let context = fresh_remote(gui, &request_id)?;
    let resource = gui
        .app_for_workflow()
        .evidence_work
        .remote
        .as_ref()
        .unwrap()
        .resources
        .get(key)
        .cloned()
        .ok_or_else(|| GuiError::not_found("Screenshot source no longer exists"))?;
    let cancelled = gui
        .app_for_workflow()
        .evidence_work
        .remote
        .as_ref()
        .unwrap()
        .cancelled
        .clone();
    Ok(RemoteImageRead {
        context,
        resource,
        request_id,
        thumbnail,
        cancelled,
    })
}
pub fn finish_remote_image(gui: &mut GuiHandle, read: &RemoteImageRead) -> GuiResult<()> {
    fresh_remote(gui, &read.request_id).map(|_| ())
}
pub fn close_remote(gui: &mut GuiHandle, request_id: &str) {
    let work = &mut gui.app_for_workflow().evidence_work;
    if !work.closed_requests.iter().any(|id| id == request_id) {
        work.closed_requests.push_back(request_id.into());
        if work.closed_requests.len() > 256 {
            work.closed_requests.pop_front();
        }
    }
    if work
        .remote
        .as_ref()
        .is_some_and(|s| s.request_id == request_id)
    {
        if let Some(scope) = &work.remote {
            scope
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        work.remote = None;
    }
}

/// Inline PR reads never replace the shared artifact/gallery selection.
pub struct PrDocumentRead {
    context: PrContext,
}
impl PrDocumentRead {
    pub fn run(&self) -> GuiResult<String> {
        let pr = crate::screenshot_sources::pr_document(&self.context, &GithubEvidence)
            .map_err(GuiError::from)?;
        Ok(pr["body"].as_str().unwrap_or("").to_owned())
    }
}
pub fn plan_pr_document(gui: &mut GuiHandle, workflow_id: &str) -> GuiResult<PrDocumentRead> {
    Ok(PrDocumentRead {
        context: crate::gui_pr_triage::screenshot_context(gui, workflow_id)?,
    })
}
pub fn finish_pr_document(gui: &mut GuiHandle, read: &PrDocumentRead) -> GuiResult<()> {
    fresh_inline(gui, &read.context)
}

pub struct InlineImageRead {
    context: PrContext,
    source: String,
}
impl InlineImageRead {
    pub fn run(&self) -> GuiResult<ImageData> {
        let _permit = crate::screenshot_evidence::image_worker().map_err(GuiError::from)?;
        crate::screenshot_sources::inline_image(&self.context, &self.source, &GithubEvidence)
            .map_err(GuiError::from)
    }
}
pub fn plan_inline_image(
    gui: &mut GuiHandle,
    workflow_id: &str,
    source: String,
) -> GuiResult<InlineImageRead> {
    if source.len() > 8192 {
        return Err(GuiError::conflict("Source URL exceeds processing limit"));
    }
    Ok(InlineImageRead {
        context: crate::gui_pr_triage::screenshot_context(gui, workflow_id)?,
        source,
    })
}
pub fn finish_inline_image(gui: &mut GuiHandle, read: &InlineImageRead) -> GuiResult<()> {
    fresh_inline(gui, &read.context)
}
fn fresh_inline(gui: &mut GuiHandle, context: &PrContext) -> GuiResult<()> {
    let current = crate::gui_pr_triage::screenshot_context(gui, &context.workflow_id)?;
    if current.number != context.number
        || current.head_sha != context.head_sha
        || current.feature_id != context.feature_id
        || current.owner != context.owner
        || current.repo != context.repo
    {
        return Err(GuiError::conflict("PR image context changed"));
    }
    Ok(())
}

pub fn open_gallery_browser(url: &str) -> GuiResult<()> {
    let parsed = crate::screenshot_sources::validated_url(url).map_err(GuiError::from)?;
    let command = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(command)
        .arg(parsed.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| GuiError::from(anyhow::Error::from(e)))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

struct CancellableGithub {
    inner: Arc<dyn EvidenceGithub>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}
impl CancellableGithub {
    fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.cancelled.load(std::sync::atomic::Ordering::Acquire),
            "Screenshot retrieval cancelled"
        );
        Ok(())
    }
}
impl EvidenceGithub for CancellableGithub {
    fn attachment_redirect(&self, context: &PrContext, source: &str) -> anyhow::Result<String> {
        self.check()?;
        self.inner.attachment_redirect(context, source)
    }
    fn json(&self, workdir: &std::path::Path, endpoint: &str) -> anyhow::Result<serde_json::Value> {
        self.check()?;
        self.inner.json(workdir, endpoint)
    }
    fn raw(
        &self,
        workdir: &std::path::Path,
        endpoint: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<u8>> {
        self.check()?;
        self.inner.raw(workdir, endpoint, limit)
    }
    fn http(
        &self,
        workdir: &std::path::Path,
        url: &str,
        auth: bool,
        limit: u64,
    ) -> anyhow::Result<crate::screenshot_sources::Download> {
        self.check()?;
        self.inner.http(workdir, url, auth, limit)
    }
}

pub struct CleanupRead {
    owner: EvidenceOwner,
}
impl CleanupRead {
    pub fn run(self) -> GuiResult<()> {
        crate::screenshot_evidence::remove_owned_directory(&self.owner).map_err(GuiError::from)
    }
}
pub fn cleanup(gui: &mut GuiHandle, scope_id: &str) -> GuiResult<()> {
    plan_cleanup(gui, scope_id)?.run()
}
