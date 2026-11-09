//! App-level layer for "new project from git URL", shared by the TUI and GUI.
//!
//! Both interfaces call the same `App` operations so they get identical
//! validation, pre-flight and clone outcomes, and both render the plain-data
//! [`CloneProjectSnapshot`] (no UI types). The mechanics live in
//! `crate::git_clone`; this module owns the single in-flight job, the
//! stale-result rule for pre-flight, and the hand-off to project registration.
//!
//! Lifetimes: a pre-flight runs on a worker thread with its own cancel token; a
//! clone is a [`CloneHandle`] that owns the destination directory. The handle
//! is only ever given up through `finalize` (registered) or rolled back, and
//! [`App::shutdown_clone_job`] covers quit/signal paths.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use serde::Serialize;

use super::App;
use crate::automation::CreateProjectRequest;
use crate::git_clone::classify::{FailureClass, FixId, advise};
use crate::git_clone::dest::{self, DestError};
use crate::git_clone::env::GitEnv;
use crate::git_clone::finalize::finalize;
use crate::git_clone::preflight::{
    self, CancelToken, CheckId, CheckStatus, PreflightOptions, PreflightReport,
};
use crate::git_clone::runner::{
    self, CloneEvent, CloneHandle, CloneOutcome, ProgressPhase, ProgressUpdate,
};
use crate::git_clone::url::{self, ParsedUrl, UrlError};

// ---------------------------------------------------------------- snapshot

/// A user-run remedy the interface may show (and copy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneFix {
    /// Stable id (`gh_auth_login`, `ssh_add`, `ssh_keygen`).
    pub id: String,
    pub label: String,
    pub command_line: String,
}

impl From<FixId> for CloneFix {
    fn from(fix: FixId) -> Self {
        let id = match fix {
            FixId::GhAuthLogin => "gh_auth_login",
            FixId::SshAdd => "ssh_add",
            FixId::SshKeygen => "ssh_keygen",
        };
        Self {
            id: id.into(),
            label: fix.label().into(),
            command_line: fix.command_line(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneCheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneCheck {
    /// `git`, `remote`, `gh_auth` or `ssh_agent`.
    pub id: String,
    pub status: CloneCheckStatus,
    pub message: String,
    pub fix: Option<CloneFix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneProgress {
    /// `counting`, `compressing`, `receiving`, `resolving`, `updating_files`
    /// or `other`.
    pub phase: String,
    pub percent: Option<u8>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClonePhase {
    Idle,
    /// Reserved for interfaces that validate asynchronously; the shared layer
    /// validates synchronously and never rests in this phase.
    #[allow(dead_code)]
    Validating,
    PreflightRunning,
    PreflightDone {
        checks: Vec<CloneCheck>,
        default_branch: Option<String>,
        /// A failed check (or an unreachable/empty remote): Start is disabled.
        blocked: bool,
    },
    Cloning {
        progress: Option<CloneProgress>,
    },
    /// Past the point of no return: cancel is refused.
    Registering,
    Succeeded {
        name: String,
        dest: PathBuf,
        branch: String,
    },
    Failed {
        message: String,
        advice: Option<String>,
        fixes: Vec<CloneFix>,
    },
    Cancelled,
}

/// Everything an interface needs to render the flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloneProjectSnapshot {
    pub phase: ClonePhase,
    pub url: Option<String>,
    pub name: Option<String>,
    pub dest: Option<PathBuf>,
    /// `start_clone` would currently be accepted (pre-flight passed or warned).
    pub can_start: bool,
    /// `cancel_clone` would currently be accepted.
    pub can_cancel: bool,
}

// ------------------------------------------------------------------ errors

/// Why a target was rejected or an operation refused. `Display` is the
/// user-facing message and is identical for both interfaces.
#[derive(Debug)]
pub enum CloneProjectError {
    Url(UrlError),
    Dest(DestError),
    EmptyName,
    NameTaken(String),
    /// A clone is in flight (cloning or registering).
    Busy,
    /// The destination is not an absolute path (after `~` expansion).
    RelativeDest(PathBuf),
    /// `start_clone` before a pre-flight for this URL has finished.
    PreflightRequired,
    PreflightBlocked,
    Start(String),
    /// The destination's parent is exactly the effective projects root and
    /// that directory does not exist. Unlike any other missing parent, the
    /// interface may offer to create it ([`App::create_projects_root`]) or to
    /// pick another root ([`App::set_projects_root`]).
    ProjectsRootMissing {
        root: PathBuf,
    },
    /// Creating or changing the projects root was refused or failed.
    ProjectsRoot(String),
}

impl fmt::Display for CloneProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(e) => write!(f, "{e}"),
            Self::Dest(e) => write!(f, "{e}"),
            Self::EmptyName => write!(f, "Enter a project name."),
            Self::NameTaken(n) => write!(
                f,
                "A project named '{n}' already exists. Choose a different name."
            ),
            Self::Busy => write!(f, "A clone is already in progress."),
            Self::RelativeDest(p) => write!(
                f,
                "The destination {} must be an absolute path (or start with ~).",
                p.display()
            ),
            Self::PreflightRequired => write!(
                f,
                "Run the pre-flight checks for this URL before starting the clone."
            ),
            Self::PreflightBlocked => write!(
                f,
                "The pre-flight checks failed; fix the problem and re-run them."
            ),
            Self::Start(m) => write!(f, "{m}"),
            Self::ProjectsRootMissing { root } => write!(
                f,
                "The projects folder {} does not exist yet. Create it, or choose a different projects folder.",
                root.display()
            ),
            Self::ProjectsRoot(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CloneProjectError {}

/// A validated clone target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneTarget {
    pub url: ParsedUrl,
    pub name: String,
    pub dest: PathBuf,
}

/// Name and directory derived from a URL (not yet validated against the store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedTarget {
    pub name: String,
    pub dest: PathBuf,
}

// --------------------------------------------------------------------- job

struct PreflightRun {
    id: u64,
    cancel: CancelToken,
    join: Option<JoinHandle<()>>,
}

struct Preflighted {
    url: String,
    branch: Option<String>,
}

struct ActiveClone {
    handle: CloneHandle,
    url: ParsedUrl,
    name: String,
    branch: Option<String>,
}

/// State of the one in-flight "new project from git URL" operation.
pub struct CloneProjectJob {
    env: GitEnv,
    /// Test-only: let `file://` fixtures through URL parsing.
    #[cfg(test)]
    allow_file_urls: bool,
    next_id: u64,
    tx: Sender<(u64, PreflightReport)>,
    rx: Receiver<(u64, PreflightReport)>,
    preflight: Option<PreflightRun>,
    /// Cancelled runs whose threads are still winding down.
    retired: Vec<JoinHandle<()>>,
    preflighted: Option<Preflighted>,
    clone: Option<ActiveClone>,
    phase: ClonePhase,
    last: Option<CloneTarget>,
    /// Harness to register the cloned project with (set by the GUI).
    preferred_agent: Option<crate::project::AgentKind>,
}

impl Default for CloneProjectJob {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            env: GitEnv::new(),
            #[cfg(test)]
            allow_file_urls: false,
            next_id: 0,
            tx,
            rx,
            preflight: None,
            retired: Vec::new(),
            preflighted: None,
            clone: None,
            phase: ClonePhase::Idle,
            last: None,
            preferred_agent: None,
        }
    }
}

fn progress_phase_name(p: ProgressPhase) -> &'static str {
    match p {
        ProgressPhase::Counting => "counting",
        ProgressPhase::Compressing => "compressing",
        ProgressPhase::Receiving => "receiving",
        ProgressPhase::Resolving => "resolving",
        ProgressPhase::UpdatingFiles => "updating_files",
        ProgressPhase::Other => "other",
    }
}

fn to_progress(p: ProgressUpdate) -> CloneProgress {
    CloneProgress {
        phase: progress_phase_name(p.phase).into(),
        percent: p.percent,
        detail: p.detail,
    }
}

fn check_id_name(id: CheckId) -> &'static str {
    match id {
        CheckId::Git => "git",
        CheckId::Remote => "remote",
        CheckId::GhAuth => "gh_auth",
        CheckId::SshAgent => "ssh_agent",
    }
}

fn failed(message: impl Into<String>) -> ClonePhase {
    ClonePhase::Failed {
        message: message.into(),
        advice: None,
        fixes: Vec::new(),
    }
}

impl CloneProjectJob {
    fn parse_url(&self, text: &str) -> Result<ParsedUrl, UrlError> {
        #[cfg(test)]
        if self.allow_file_urls
            && let Some(rest) = text.trim().strip_prefix("file://")
        {
            let last = rest.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            return Ok(ParsedUrl {
                raw: text.trim().to_string(),
                kind: url::UrlKind::Https,
                host: "localhost".into(),
                port: None,
                repo_name: url::derive_repo_name(last)?,
            });
        }
        url::parse(text)
    }

    fn clone_in_flight(&self) -> bool {
        self.clone.is_some()
    }

    /// Cancel the running pre-flight (if any) without waiting for its thread.
    fn retire_preflight(&mut self) {
        if let Some(mut run) = self.preflight.take() {
            run.cancel.store(true, Ordering::SeqCst);
            if let Some(j) = run.join.take() {
                self.retired.push(j);
            }
        }
        self.retired.retain(|j| !j.is_finished());
    }

    fn can_start(&self) -> bool {
        !self.clone_in_flight()
            && matches!(self.phase, ClonePhase::PreflightDone { blocked: false, .. })
    }

    fn can_cancel(&self) -> bool {
        match self.phase {
            ClonePhase::PreflightRunning => self.preflight.is_some(),
            ClonePhase::Cloning { .. } => self.clone.is_some(),
            _ => false,
        }
    }

    /// Idempotent teardown for quit/signal paths.
    fn shutdown(&mut self) {
        if let Some(run) = self.preflight.as_mut() {
            run.cancel.store(true, Ordering::SeqCst);
            if let Some(j) = run.join.take() {
                let _ = j.join();
            }
        }
        self.preflight = None;
        for j in self.retired.drain(..) {
            let _ = j.join();
        }
        if let Some(mut active) = self.clone.take() {
            // Not yet released (registration never ran): stop the process
            // group and remove the directory we created.
            let _ = active.handle.rollback();
            self.phase = ClonePhase::Cancelled;
        } else if matches!(self.phase, ClonePhase::PreflightRunning) {
            self.phase = ClonePhase::Cancelled;
        }
    }
}

impl Drop for CloneProjectJob {
    fn drop(&mut self) {
        // `ActiveClone` drops its handle, which cleans up; this additionally
        // stops a pre-flight child promptly.
        self.shutdown();
    }
}

// --------------------------------------------------------------- App layer

impl App {
    fn clone_job(&mut self) -> &mut CloneProjectJob {
        self.clone_project.get_or_insert_with(Default::default)
    }

    /// Test-only: use `env` (fake git, file transport) and accept `file://`.
    #[cfg(test)]
    pub(crate) fn set_clone_project_env_for_test(&mut self, env: GitEnv, allow_file_urls: bool) {
        let job = self.clone_job();
        job.env = env;
        job.allow_file_urls = allow_file_urls;
    }

    /// Test-only: put the job in `phase`, as if a pre-flight for
    /// `preflighted_url` had just produced it (lets UI tests reach states such
    /// as `Registering` that are momentary in a real run).
    #[cfg(test)]
    pub(crate) fn set_clone_phase_for_test(
        &mut self,
        phase: ClonePhase,
        preflighted_url: Option<&str>,
    ) {
        let job = self.clone_job();
        job.phase = phase;
        job.preflighted = preflighted_url.map(|u| Preflighted {
            url: u.into(),
            branch: None,
        });
    }

    /// Derive the project name and directory (`<projects root>/<repo>`) from a
    /// URL typed by the user.
    pub fn derive_clone_target(
        &mut self,
        url_text: &str,
    ) -> Result<DerivedTarget, CloneProjectError> {
        let parsed = self
            .clone_job()
            .parse_url(url_text)
            .map_err(CloneProjectError::Url)?;
        let root = self.config.projects_root();
        Ok(DerivedTarget {
            dest: dest::destination(&root, &parsed.repo_name),
            name: parsed.repo_name,
        })
    }

    /// Validate a (possibly user-edited) target: URL, project name uniqueness
    /// and destination (not existing, not registered, parent writable).
    pub fn validate_clone_target(
        &mut self,
        url_text: &str,
        dest: &Path,
        name: &str,
    ) -> Result<CloneTarget, CloneProjectError> {
        let parsed = self
            .clone_job()
            .parse_url(url_text)
            .map_err(CloneProjectError::Url)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(CloneProjectError::EmptyName);
        }
        if self.store.find_project(name).is_some() {
            return Err(CloneProjectError::NameTaken(name.to_string()));
        }
        let dest = super::expand_tilde(&dest.to_string_lossy(), dirs::home_dir());
        if !dest.is_absolute() {
            return Err(CloneProjectError::RelativeDest(dest));
        }
        let registered: Vec<PathBuf> = self.store.projects.iter().map(|p| p.repo.clone()).collect();
        if let Err(e) = dest::validate(&dest, &registered) {
            return Err(match e {
                DestError::ParentMissing(parent) if parent == self.config.projects_root() => {
                    CloneProjectError::ProjectsRootMissing { root: parent }
                }
                other => CloneProjectError::Dest(other),
            });
        }
        Ok(CloneTarget {
            url: parsed,
            name: name.to_string(),
            dest,
        })
    }

    /// `Some(root)` when `dest`'s parent is exactly the effective projects
    /// root and that directory does not exist, i.e. when validation would
    /// report [`CloneProjectError::ProjectsRootMissing`]. Cheap (one `stat`).
    pub fn projects_root_status(&self, dest: &Path) -> Option<PathBuf> {
        let dest = super::expand_tilde(&dest.to_string_lossy(), dirs::home_dir());
        let root = self.config.projects_root();
        if dest.parent() != Some(root.as_path()) {
            return None;
        }
        match std::fs::metadata(&root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(root),
            _ => None,
        }
    }

    /// Create the effective projects root (and only it) after the user has
    /// confirmed. Takes no path, so it cannot be steered at anything else.
    /// Refuses when anything already exists at the path (directory, file or
    /// symlink: an existing directory is never touched) and while a clone is
    /// in flight. Never deletes anything.
    pub fn create_projects_root(&mut self) -> Result<PathBuf, CloneProjectError> {
        if self.clone_job().clone_in_flight() {
            return Err(CloneProjectError::Busy);
        }
        let root = self.config.projects_root();
        match std::fs::symlink_metadata(&root) {
            Ok(meta) => {
                let what = if meta.is_dir() {
                    "already exists"
                } else if meta.file_type().is_symlink() {
                    "is a symbolic link; it was left untouched"
                } else {
                    "exists but is not a directory"
                };
                return Err(CloneProjectError::ProjectsRoot(format!(
                    "The projects folder {} {what}. Nothing was created.",
                    root.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CloneProjectError::ProjectsRoot(format!(
                    "Could not inspect the projects folder {}: {e}",
                    root.display()
                )));
            }
        }
        std::fs::create_dir_all(&root).map_err(|e| {
            CloneProjectError::ProjectsRoot(format!(
                "Could not create the projects folder {}: {e}",
                root.display()
            ))
        })?;
        Ok(root)
    }

    /// Change and persist the projects root. `path` is stored as typed
    /// (trimmed, `~` unexpanded). It must be absolute or start with `~/`, and
    /// if it exists it must be a directory; a nonexistent root is allowed (the
    /// interface then offers [`create_projects_root`](Self::create_projects_root)).
    /// Returns the effective (expanded) path. Re-derive the destination with
    /// [`derive_clone_target`](Self::derive_clone_target) afterwards.
    pub fn set_projects_root(&mut self, path: &str) -> Result<PathBuf, CloneProjectError> {
        if self.clone_job().clone_in_flight() {
            return Err(CloneProjectError::Busy);
        }
        let text = path.trim();
        if text.is_empty() {
            return Err(CloneProjectError::ProjectsRoot(
                "Enter a projects folder.".into(),
            ));
        }
        let effective = super::expand_tilde(text, dirs::home_dir());
        if !effective.is_absolute() {
            return Err(CloneProjectError::ProjectsRoot(format!(
                "{text} is not an absolute path. Use a full path or one starting with ~/."
            )));
        }
        match std::fs::symlink_metadata(&effective) {
            Ok(_) => {
                if !std::fs::metadata(&effective).is_ok_and(|m| m.is_dir()) {
                    return Err(CloneProjectError::ProjectsRoot(format!(
                        "{} exists but is not a directory.",
                        effective.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CloneProjectError::ProjectsRoot(format!(
                    "Could not inspect {}: {e}",
                    effective.display()
                )));
            }
        }
        let previous = self.config.projects.projects_root.replace(text.to_string());
        if let Err(e) = self.try_save_config() {
            self.config.projects.projects_root = previous;
            return Err(CloneProjectError::ProjectsRoot(format!(
                "Could not save the projects folder setting: {e:#}"
            )));
        }
        Ok(self.config.projects_root())
    }

    /// Validate, then run the pre-flight checks on a worker thread. The result
    /// is applied by [`poll_clone_preflight`](Self::poll_clone_preflight).
    pub fn start_clone_preflight(
        &mut self,
        url_text: &str,
        dest: &Path,
        name: &str,
    ) -> Result<(), CloneProjectError> {
        if self.clone_job().clone_in_flight() {
            return Err(CloneProjectError::Busy);
        }
        let target = self.validate_clone_target(url_text, dest, name)?;
        let job = self.clone_job();
        job.retire_preflight();
        job.preflighted = None;
        job.next_id += 1;
        let id = job.next_id;
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let opts = PreflightOptions {
            env: job.env.clone(),
            ..PreflightOptions::default()
        };
        let tx = job.tx.clone();
        let url = target.url.clone();
        let token = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("clone-preflight".into())
            .spawn(move || {
                let report = preflight::run(&url, &opts, &token);
                let _ = tx.send((id, report));
            });
        match spawned {
            Ok(join) => {
                job.preflight = Some(PreflightRun {
                    id,
                    cancel,
                    join: Some(join),
                });
                job.phase = ClonePhase::PreflightRunning;
                job.last = Some(target);
                Ok(())
            }
            Err(e) => Err(CloneProjectError::Start(format!(
                "could not start the pre-flight checks: {e}"
            ))),
        }
    }

    /// Apply a finished pre-flight if it belongs to the current request.
    /// Returns whether the snapshot changed.
    pub fn poll_clone_preflight(&mut self) -> bool {
        let Some(job) = self.clone_project.as_mut() else {
            return false;
        };
        let mut changed = false;
        while let Ok((id, report)) = job.rx.try_recv() {
            // Stale-result rule: only the current request may apply.
            if job.preflight.as_ref().map(|r| r.id) != Some(id) {
                continue;
            }
            if let Some(mut run) = job.preflight.take()
                && let Some(j) = run.join.take()
            {
                let _ = j.join();
            }
            changed = true;
            if report.outcome == preflight::Outcome::Cancelled {
                job.phase = ClonePhase::Cancelled;
                continue;
            }
            let blocked = !report.can_start();
            let checks = report
                .checks
                .iter()
                .map(|c| CloneCheck {
                    id: check_id_name(c.id).into(),
                    status: match c.status {
                        CheckStatus::Pass => CloneCheckStatus::Pass,
                        CheckStatus::Warn => CloneCheckStatus::Warn,
                        CheckStatus::Fail => CloneCheckStatus::Fail,
                    },
                    message: c.message.clone(),
                    fix: c.fix.map(CloneFix::from),
                })
                .collect();
            job.preflighted = job.last.as_ref().map(|t| Preflighted {
                url: t.url.raw.clone(),
                branch: report.default_branch.clone(),
            });
            job.phase = ClonePhase::PreflightDone {
                checks,
                default_branch: report.default_branch,
                blocked,
            };
        }
        changed
    }

    /// Re-validate and start cloning. Requires a finished, unblocked
    /// pre-flight for the same URL.
    /// Harness the next successful clone is registered with (`None` = default).
    pub fn set_clone_preferred_agent(&mut self, agent: Option<crate::project::AgentKind>) {
        self.clone_job().preferred_agent = agent;
    }

    pub fn start_clone(
        &mut self,
        url_text: &str,
        dest: &Path,
        name: &str,
    ) -> Result<(), CloneProjectError> {
        if self.clone_job().clone_in_flight() {
            return Err(CloneProjectError::Busy);
        }
        let target = self.validate_clone_target(url_text, dest, name)?;
        let job = self.clone_job();
        match (&job.phase, &job.preflighted) {
            (ClonePhase::PreflightDone { blocked: false, .. }, Some(p))
                if p.url == target.url.raw => {}
            (ClonePhase::PreflightDone { blocked: true, .. }, Some(p))
                if p.url == target.url.raw =>
            {
                return Err(CloneProjectError::PreflightBlocked);
            }
            _ => return Err(CloneProjectError::PreflightRequired),
        }
        let branch = job.preflighted.as_ref().and_then(|p| p.branch.clone());
        let handle = runner::start(&job.env, &target.url.raw, &target.dest)
            .map_err(|e| CloneProjectError::Start(e.to_string()))?;
        job.phase = ClonePhase::Cloning { progress: None };
        job.clone = Some(ActiveClone {
            handle,
            url: target.url.clone(),
            name: target.name.clone(),
            branch,
        });
        job.last = Some(target);
        Ok(())
    }

    /// Drain clone events; on success finalize (register) automatically.
    /// Returns whether the snapshot changed.
    pub fn poll_clone_progress(&mut self) -> bool {
        let Some(job) = self.clone_project.as_mut() else {
            return false;
        };
        let Some(active) = job.clone.as_mut() else {
            return false;
        };
        let mut changed = false;
        let mut outcome = None;
        loop {
            match active.handle.try_recv() {
                Ok(CloneEvent::Progress(p)) => {
                    job.phase = ClonePhase::Cloning {
                        progress: Some(to_progress(p)),
                    };
                    changed = true;
                }
                Ok(CloneEvent::Finished(o)) => {
                    outcome = Some(o);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    outcome = Some(active.handle.wait());
                    break;
                }
            }
        }
        let Some(outcome) = outcome else {
            return changed;
        };
        match outcome {
            CloneOutcome::Success => self.register_cloned_project(),
            CloneOutcome::Cancelled => {
                job.clone = None;
                job.phase = ClonePhase::Cancelled;
            }
            CloneOutcome::Failed { class, stderr } => {
                let url = active.url.clone();
                job.clone = None;
                job.phase = failure_phase(&class, &stderr, &url);
            }
        }
        true
    }

    /// Success path: finalize with registration through the one existing
    /// project-creation path. Failure rolls the directory back.
    fn register_cloned_project(&mut self) {
        let (env, mut active) = {
            let job = self.clone_job();
            let Some(active) = job.clone.take() else {
                return;
            };
            job.phase = ClonePhase::Registering;
            (job.env.clone(), active)
        };
        let preferred_agent = self.clone_job().preferred_agent.take();
        let name = active.name.clone();
        let result = finalize(
            &env,
            &mut active.handle,
            active.branch.as_deref(),
            &name,
            |path, name| {
                self.create_project_from_request(&CreateProjectRequest {
                    path: path.to_path_buf(),
                    project_name: name.to_string(),
                    preferred_agent: preferred_agent.clone(),
                    dry_run: false,
                })
                .map(|_| ())
            },
        );
        let phase = match result {
            Ok(done) => ClonePhase::Succeeded {
                name,
                dest: done.dest,
                branch: done.branch,
            },
            Err(e) => failed(format!("{e:#}")),
        };
        self.clone_job().phase = phase;
    }

    /// Ask the current step to stop. `false` when there is nothing to cancel
    /// or it is too late (registration has begun, or the clone already
    /// finished and is being finalized).
    pub fn cancel_clone(&mut self) -> bool {
        let job = self.clone_job();
        match job.phase {
            ClonePhase::Registering => false,
            ClonePhase::Cloning { .. } => job.clone.as_ref().is_some_and(|c| c.handle.cancel()),
            ClonePhase::PreflightRunning if job.preflight.is_some() => {
                job.retire_preflight();
                job.phase = ClonePhase::Cancelled;
                true
            }
            _ => false,
        }
    }

    /// Return to `Idle` (e.g. when the form is dismissed). Refused while a
    /// clone is in flight; cancel it first.
    #[allow(dead_code)] // first caller is the TUI/GUI form
    pub fn reset_clone_project(&mut self) -> bool {
        let Some(job) = self.clone_project.as_mut() else {
            return true;
        };
        if job.clone_in_flight() {
            return false;
        }
        job.retire_preflight();
        job.preflighted = None;
        job.last = None;
        job.phase = ClonePhase::Idle;
        true
    }

    /// Plain-data view of the flow for rendering.
    pub fn clone_project_snapshot(&self) -> CloneProjectSnapshot {
        match self.clone_project.as_ref() {
            None => CloneProjectSnapshot {
                phase: ClonePhase::Idle,
                url: None,
                name: None,
                dest: None,
                can_start: false,
                can_cancel: false,
            },
            Some(job) => CloneProjectSnapshot {
                phase: job.phase.clone(),
                url: job.last.as_ref().map(|t| t.url.raw.clone()),
                name: job.last.as_ref().map(|t| t.name.clone()),
                dest: job.last.as_ref().map(|t| t.dest.clone()),
                can_start: job.can_start(),
                can_cancel: job.can_cancel(),
            },
        }
    }

    /// Stop everything the clone flow owns: cancel pre-flight, kill a running
    /// clone's process group and remove the directory it created. Idempotent;
    /// called from TUI teardown/signal handling and the GUI exit hook.
    pub fn shutdown_clone_job(&mut self) {
        if let Some(job) = self.clone_project.as_mut() {
            job.shutdown();
        }
    }
}

fn failure_phase(class: &FailureClass, stderr: &str, url: &ParsedUrl) -> ClonePhase {
    let advice = advise(class, url);
    let mut message = advice.summary;
    // A non-empty raw message is already the advice detail; only fall back to
    // the full stderr when the classifier kept nothing.
    if let FailureClass::Unknown(raw) = class {
        let stderr = stderr.trim();
        if raw.is_empty() && !stderr.is_empty() {
            message = format!("{message} {stderr}");
        }
    }
    ClonePhase::Failed {
        message,
        advice: (!advice.detail.is_empty()).then_some(advice.detail),
        fixes: advice.fixes.into_iter().map(CloneFix::from).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Project, ProjectStore};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn test_app() -> App {
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(|p| Ok(p.to_path_buf()));
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(worktree),
        );
        app.store_path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        app
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    /// Bare repo whose default branch is `trunk`; returns its `file://` URL.
    fn trunk_remote(root: &Path) -> String {
        let bare = root.join("remote.git");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(root, &["init", "--bare", "-b", "trunk", "remote.git"]);
        git(&work, &["init", "-b", "trunk"]);
        std::fs::write(work.join("README"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "init"]);
        git(&work, &["push", bare.to_str().unwrap(), "trunk"]);
        format!("file://{}", bare.display())
    }

    fn file_app() -> App {
        let mut app = test_app();
        app.set_clone_project_env_for_test(GitEnv::new().with_file_transport(true), true);
        app
    }

    fn wait_until(app: &mut App, what: &str, done: impl Fn(&ClonePhase) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            app.poll_clone_preflight();
            app.poll_clone_progress();
            if done(&app.clone_project_snapshot().phase) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn derive_uses_projects_root() {
        let mut app = test_app();
        app.config.projects.projects_root = Some("/work/repos".into());
        let t = app
            .derive_clone_target("https://github.com/o/widget.git")
            .unwrap();
        assert_eq!(t.name, "widget");
        assert_eq!(t.dest, PathBuf::from("/work/repos/widget"));
        assert!(matches!(
            app.derive_clone_target("file:///x"),
            Err(CloneProjectError::Url(_))
        ));
    }

    #[test]
    fn duplicate_name_and_registered_path_block_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = test_app();
        let existing = tmp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        app.store.add_project(Project::new(
            "widget".into(),
            existing.clone(),
            true,
            Default::default(),
        ));
        let url = "https://github.com/o/widget.git";
        let dest = tmp.path().join("widget");

        let err = app.validate_clone_target(url, &dest, "widget").unwrap_err();
        assert!(matches!(err, CloneProjectError::NameTaken(_)), "{err}");
        assert!(matches!(
            app.validate_clone_target(url, &dest, "  ").unwrap_err(),
            CloneProjectError::EmptyName
        ));
        let err = app
            .validate_clone_target(url, &existing, "other")
            .unwrap_err();
        assert!(matches!(err, CloneProjectError::Dest(_)), "{err}");
        assert!(app.validate_clone_target(url, &dest, "other").is_ok());
        // Preflight refuses the same inputs before spawning anything.
        assert!(app.start_clone_preflight(url, &dest, "widget").is_err());
        assert_eq!(app.clone_project_snapshot().phase, ClonePhase::Idle);
    }

    #[test]
    fn stale_preflight_result_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let mut app = file_app();
        let dest = tmp.path().join("repo");
        app.start_clone_preflight(&url, &dest, "repo").unwrap();
        let job = app.clone_job();
        let current = job.preflight.as_ref().unwrap().id;
        // A late answer from an older request.
        job.tx
            .send((
                current - 1,
                PreflightReport {
                    outcome: preflight::Outcome::Failed,
                    checks: vec![],
                    default_branch: Some("stale".into()),
                },
            ))
            .unwrap();
        wait_until(&mut app, "preflight", |p| {
            matches!(p, ClonePhase::PreflightDone { .. })
        });
        match app.clone_project_snapshot().phase {
            ClonePhase::PreflightDone {
                default_branch,
                blocked,
                ..
            } => {
                assert_eq!(default_branch.as_deref(), Some("trunk"));
                assert!(!blocked);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn happy_path_registers_one_project_on_the_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let mut app = file_app();
        let dest = tmp.path().join("repo");

        assert!(matches!(
            app.start_clone(&url, &dest, "repo"),
            Err(CloneProjectError::PreflightRequired)
        ));
        app.start_clone_preflight(&url, &dest, "repo").unwrap();
        wait_until(&mut app, "preflight", |p| {
            matches!(p, ClonePhase::PreflightDone { .. })
        });
        assert!(app.clone_project_snapshot().can_start);
        app.start_clone(&url, &dest, "repo").unwrap();
        assert!(matches!(
            app.start_clone(&url, &dest, "repo"),
            Err(CloneProjectError::Busy)
        ));
        wait_until(&mut app, "clone", |p| {
            matches!(p, ClonePhase::Succeeded { .. } | ClonePhase::Failed { .. })
        });
        match app.clone_project_snapshot().phase {
            ClonePhase::Succeeded { name, branch, .. } => {
                assert_eq!(name, "repo");
                assert_eq!(branch, "trunk");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(app.store.projects.len(), 1);
        assert_eq!(app.store.projects[0].name, "repo");
        assert!(dest.join("README").is_file());
        let head = Command::new("git")
            .args([
                "-C",
                dest.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "trunk");
        // Releasing ownership: shutdown must not delete a registered project.
        app.shutdown_clone_job();
        assert!(dest.join("README").is_file());
    }

    #[test]
    fn registration_failure_removes_dir_and_leaves_no_record() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let mut app = file_app();
        let dest = tmp.path().join("repo");
        app.start_clone_preflight(&url, &dest, "repo").unwrap();
        wait_until(&mut app, "preflight", |p| {
            matches!(p, ClonePhase::PreflightDone { .. })
        });
        app.start_clone(&url, &dest, "repo").unwrap();
        // A conflicting project appears after validation.
        let other = tmp.path().join("other");
        std::fs::create_dir(&other).unwrap();
        app.store
            .add_project(Project::new("repo".into(), other, true, Default::default()));
        wait_until(&mut app, "clone", |p| {
            matches!(p, ClonePhase::Succeeded { .. } | ClonePhase::Failed { .. })
        });
        match app.clone_project_snapshot().phase {
            ClonePhase::Failed { message, .. } => {
                assert!(message.contains("already exists"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        assert!(!dest.exists(), "owned dir must be removed");
        assert_eq!(app.store.projects.len(), 1, "only the conflicting project");
        assert_eq!(app.store.projects[0].repo, tmp.path().join("other"));
    }

    fn pid_gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let r = unsafe { libc::kill(pid, 0) };
            if r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Fake git that reports progress and forks a long-sleeping grandchild.
    fn slow_clone_app(tmp: &Path) -> (App, PathBuf) {
        let pidfile = tmp.join("gc.pid");
        let bin = tmp.join("fakegit");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\nsleep 300 &\necho $! > '{}'\n\
                 printf 'Receiving objects:  10%% (1/10)\\r' >&2\nwait\n",
                pidfile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut app = test_app();
        app.set_clone_project_env_for_test(GitEnv::new().with_git_binary(bin), false);
        (app, pidfile)
    }

    fn arm_for_start(app: &mut App, url: &str) {
        let job = app.clone_job();
        job.phase = ClonePhase::PreflightDone {
            checks: vec![],
            default_branch: None,
            blocked: false,
        };
        job.preflighted = Some(Preflighted {
            url: url.into(),
            branch: None,
        });
    }

    fn start_slow(tmp: &Path) -> (App, PathBuf, i32) {
        let (mut app, pidfile) = slow_clone_app(tmp);
        let url = "https://example.com/o/repo.git";
        let dest = tmp.join("repo");
        arm_for_start(&mut app, url);
        app.start_clone(url, &dest, "repo").unwrap();
        wait_until(&mut app, "progress", |p| {
            matches!(p, ClonePhase::Cloning { progress: Some(_) })
        });
        let gc = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        (app, dest, gc)
    }

    #[test]
    fn shutdown_mid_clone_leaves_no_dir_and_no_process() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, dest, gc) = start_slow(tmp.path());
        assert!(dest.is_dir());
        let pgid = app.clone_job().clone.as_ref().unwrap().handle.pgid();
        app.shutdown_clone_job();
        assert!(!dest.exists());
        assert!(pid_gone(gc), "grandchild survived shutdown");
        assert!(pid_gone(-pgid), "process group survived shutdown");
        assert!(app.store.projects.is_empty());
        assert_eq!(app.clone_project_snapshot().phase, ClonePhase::Cancelled);
        app.shutdown_clone_job(); // idempotent
    }

    #[test]
    fn cancel_mid_clone_is_applied_by_polling() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, dest, gc) = start_slow(tmp.path());
        assert!(app.clone_project_snapshot().can_cancel);
        assert!(app.cancel_clone());
        wait_until(&mut app, "cancel", |p| *p == ClonePhase::Cancelled);
        assert!(!dest.exists());
        assert!(pid_gone(gc));
    }

    #[test]
    fn cancel_is_refused_while_registering() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, dest, _gc) = start_slow(tmp.path());
        app.clone_job().phase = ClonePhase::Registering;
        assert!(!app.cancel_clone());
        assert!(dest.is_dir(), "refused cancel must not touch the clone");
        assert!(!app.clone_project_snapshot().can_cancel);
        // Cleanup path still works.
        app.shutdown_clone_job();
        assert!(!dest.exists());
    }

    #[test]
    fn shutdown_during_preflight_cancels_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, _pidfile) = slow_clone_app(tmp.path());
        // The fake git never answers ls-remote, so the pre-flight hangs.
        app.start_clone_preflight("https://example.com/o/repo.git", &tmp.path().join("r"), "r")
            .unwrap();
        assert_eq!(
            app.clone_project_snapshot().phase,
            ClonePhase::PreflightRunning
        );
        app.shutdown_clone_job();
        assert_eq!(app.clone_project_snapshot().phase, ClonePhase::Cancelled);
        assert!(!app.poll_clone_preflight());
    }

    // ------------------------------------------------ projects root

    fn root_app(root: &Path) -> App {
        let mut app = test_app();
        app.config.projects.projects_root = Some(root.to_string_lossy().into_owned());
        app
    }

    const ROOT_URL: &str = "https://github.com/o/widget.git";

    #[test]
    fn missing_projects_root_is_a_distinct_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("code");
        let mut app = root_app(&root);
        let dest = root.join("widget");
        match app.validate_clone_target(ROOT_URL, &dest, "widget") {
            Err(CloneProjectError::ProjectsRootMissing { root: r }) => assert_eq!(r, root),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            app.start_clone_preflight(ROOT_URL, &dest, "widget"),
            Err(CloneProjectError::ProjectsRootMissing { .. })
        ));
        assert!(matches!(
            app.start_clone(ROOT_URL, &dest, "widget"),
            Err(CloneProjectError::ProjectsRootMissing { .. })
        ));
        assert_eq!(app.projects_root_status(&dest), Some(root.clone()));
        // Trailing slash on the configured root still matches.
        app.config.projects.projects_root = Some(format!("{}/", root.display()));
        assert_eq!(app.projects_root_status(&dest), Some(root.clone()));
        assert!(!root.exists(), "validation never creates anything");
    }

    #[test]
    fn missing_non_root_parent_keeps_the_old_block() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("code");
        let mut app = root_app(&root);
        let dest = tmp.path().join("elsewhere").join("widget");
        assert!(matches!(
            app.validate_clone_target(ROOT_URL, &dest, "widget"),
            Err(CloneProjectError::Dest(DestError::ParentMissing(_)))
        ));
        assert_eq!(app.projects_root_status(&dest), None);
        // A deeper path under a missing root is not the root either.
        let deep = root.join("sub").join("widget");
        assert!(matches!(
            app.validate_clone_target(ROOT_URL, &deep, "widget"),
            Err(CloneProjectError::Dest(DestError::ParentMissing(_)))
        ));
        assert_eq!(app.projects_root_status(&deep), None);
    }

    #[test]
    fn create_projects_root_creates_only_the_root_then_validation_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let root = tmp.path().join("code");
        let mut app = root_app(&root);
        let dest = root.join("widget");
        assert_eq!(app.create_projects_root().unwrap(), root);
        assert!(root.is_dir());
        assert!(!dest.exists());
        assert!(app.validate_clone_target(ROOT_URL, &dest, "widget").is_ok());
        assert_eq!(app.projects_root_status(&dest), None);
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        // Existing directory: refused, untouched.
        let inner = root.join("mine");
        std::fs::write(&inner, "x").unwrap();
        let err = app.create_projects_root().unwrap_err();
        assert!(matches!(err, CloneProjectError::ProjectsRoot(_)), "{err}");
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(std::fs::read_to_string(&inner).unwrap(), "x");
    }

    #[test]
    fn create_projects_root_refuses_file_and_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("afile");
        std::fs::write(&file, "data").unwrap();
        let mut app = root_app(&file);
        let err = app.create_projects_root().unwrap_err();
        assert!(err.to_string().contains("not a directory"), "{err}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "data");

        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        app.config.projects.projects_root = Some(link.to_string_lossy().into_owned());
        assert!(app.create_projects_root().is_err());
        let dangling = tmp.path().join("dangling");
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), &dangling).unwrap();
        app.config.projects.projects_root = Some(dangling.to_string_lossy().into_owned());
        assert!(app.create_projects_root().is_err());
        assert!(!tmp.path().join("nowhere").exists());
        assert!(std::fs::symlink_metadata(&dangling).is_ok());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "data");
    }

    #[test]
    fn projects_root_changes_are_refused_during_a_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, dest, _gc) = start_slow(tmp.path());
        let root = tmp.path().join("newroot");
        app.config.projects.projects_root = Some(root.to_string_lossy().into_owned());
        assert!(matches!(
            app.create_projects_root(),
            Err(CloneProjectError::Busy)
        ));
        assert!(!root.exists());
        assert!(matches!(
            app.set_projects_root("/somewhere/else"),
            Err(CloneProjectError::Busy)
        ));
        assert_eq!(
            app.config.projects.projects_root.as_deref(),
            Some(root.to_str().unwrap())
        );
        assert!(dest.is_dir(), "the running clone is untouched");
        app.shutdown_clone_job();
    }

    #[test]
    fn set_projects_root_validates_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = test_app();
        for bad in ["", "   ", "relative/dir", "~someone/x"] {
            assert!(
                matches!(
                    app.set_projects_root(bad),
                    Err(CloneProjectError::ProjectsRoot(_))
                ),
                "{bad:?}"
            );
        }
        let file = tmp.path().join("f");
        std::fs::write(&file, "x").unwrap();
        let err = app.set_projects_root(file.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("not a directory"), "{err}");
        assert_eq!(
            app.config.projects.projects_root, None,
            "rejects change nothing"
        );

        // Nonexistent is allowed and is not created.
        let missing = tmp.path().join("fresh");
        let eff = app
            .set_projects_root(&format!("  {}  ", missing.display()))
            .unwrap();
        assert_eq!(eff, missing);
        assert!(!missing.exists());
        assert_eq!(
            app.config.projects.projects_root.as_deref(),
            Some(missing.to_str().unwrap()),
            "stored trimmed"
        );
        // Existing dir; tilde stored unexpanded.
        let eff = app.set_projects_root("~/some-amf-root").unwrap();
        assert_eq!(
            app.config.projects.projects_root.as_deref(),
            Some("~/some-amf-root")
        );
        assert_eq!(eff, dirs::home_dir().unwrap().join("some-amf-root"));

        // Round-trips through the saved config file.
        let existing = tmp.path().join("exists");
        std::fs::create_dir(&existing).unwrap();
        app.set_projects_root(existing.to_str().unwrap()).unwrap();
        let cfg = crate::project::amf_config_dir().join("config.json");
        let loaded: crate::app::AppConfig =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(loaded.projects_root(), existing);
        assert_eq!(app.config.projects_root(), existing);
    }
}
