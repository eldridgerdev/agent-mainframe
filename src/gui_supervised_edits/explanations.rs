//! Owned, call-free preparation and cancellable explanation work. No AppMode
//! transition or hook reply belongs here: explanations never decide an edit.
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{pending_for_feature, resolve};
use crate::app::edit_explanation::{ExplanationCall, ExplanationPreparation};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};

const MAX_TEXT: usize = 1024 * 1024;
type Runner = fn(&Path, &ExplanationCall, &AtomicBool) -> anyhow::Result<String>;
fn run(workdir: &Path, call: &ExplanationCall, cancelled: &AtomicBool) -> anyhow::Result<String> {
    crate::headless::HeadlessRunner::run_edit_explanation(
        workdir,
        &call.prompt,
        crate::headless::ModelSel::new(call.model.as_deref(), call.reasoning),
        cancelled,
    )
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ExplanationView {
    SavedNote {
        text: String,
    },
    Precall {
        owner: String,
        prompt: String,
        model: Option<String>,
        reasoning: Option<crate::headless::ReasoningLevel>,
    },
    Running {
        owner: String,
        request: u64,
    },
    Complete {
        owner: String,
        request: u64,
        text: String,
    },
    Failed {
        owner: String,
        request: u64,
        message: String,
    },
    Cancelled {
        owner: String,
    },
}

#[derive(Clone)]
struct Source {
    target: FeatureTarget,
    edit: String,
    revision: String,
    workdir: PathBuf,
    identity: String,
    filesystem: String,
    is_git: bool,
    call: ExplanationCall,
}
struct Preparation {
    owner: String,
    source: Source,
}
struct Job {
    owner: String,
    request: u64,
    source: Source,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<String, String>>,
    finished: Option<Result<String, String>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Managed independently of GuiHandle so a worker cannot replace an underlying
/// Review/Learning/planning mode. Each desktop instance owns its own controller.
pub struct Controller {
    preparation: Option<Preparation>,
    job: Option<Job>,
    sequence: u64,
    config_path: PathBuf,
    runner: Runner,
    available: fn() -> bool,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            preparation: None,
            job: None,
            sequence: 0,
            config_path: crate::project::amf_config_dir().join("config.json"),
            runner: run,
            available: crate::headless::HeadlessRunner::edit_explanation_available,
        }
    }
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
fn directory(path: &Path) -> GuiResult<serde_json::Value> {
    let canonical = path.canonicalize().map_err(anyhow::Error::from)?;
    let metadata = canonical.metadata().map_err(anyhow::Error::from)?;
    if !metadata.is_dir() {
        return Err(GuiError::conflict("The checkout is no longer a directory"));
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created().ok());
    Ok(serde_json::json!([canonical, identity]))
}
fn git(workdir: &Path, args: &[&str]) -> GuiResult<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .output()
        .map_err(anyhow::Error::from)?;
    if !out.status.success() {
        return Err(GuiError::conflict(
            "Cannot observe the checkout's Git identity",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn filesystem(
    workdir: &Path,
    files: &[PathBuf],
    waiting: &[PathBuf],
    is_git: bool,
) -> GuiResult<String> {
    let mut inputs = vec![directory(workdir)?];
    for path in waiting {
        inputs.push(directory(path)?);
    }
    if is_git {
        let gitdir = PathBuf::from(git(workdir, &["rev-parse", "--absolute-git-dir"])?);
        inputs.extend([
            directory(&gitdir)?,
            serde_json::json!(git(workdir, &["rev-parse", "--verify", "HEAD"])?),
            serde_json::json!(std::fs::read(gitdir.join("HEAD")).map_err(anyhow::Error::from)?),
        ]);
    }
    for file in files {
        // Missing notification/captured files cannot inherit a prior approval.
        inputs.push(serde_json::json!([
            file,
            hash(std::fs::read(file).map_err(anyhow::Error::from)?)
        ]));
    }
    Ok(hash(
        serde_json::to_vec(&inputs).map_err(anyhow::Error::from)?,
    ))
}

fn project_configuration(repo: &Path) -> GuiResult<crate::extension::ExtensionConfig> {
    // Validate only the effective file. Missing optional config keeps shared
    // defaults; malformed/unreadable config must not enable a paid fallback.
    let (mut value, revision) =
        crate::gui_settings::read_json_document(&crate::extension::project_config_path(repo))?;
    if revision == "missing" {
        value = crate::gui_settings::read_json_document(
            &crate::extension::legacy_project_config_path(repo),
        )?
        .0;
    }
    Ok(serde_json::from_value(value).map_err(anyhow::Error::from)?)
}

fn allowed_explanation_agents(
    project: &crate::extension::ExtensionConfig,
    global: &crate::extension::ExtensionConfig,
) -> Vec<crate::project::AgentKind> {
    crate::project::AgentKind::allowed_list(
        project
            .allowed_agents
            .as_deref()
            .or(global.allowed_agents.as_deref()),
    )
}

struct Observed {
    source: Option<Source>,
    note: Option<String>,
    files: Vec<PathBuf>,
    waiting: Vec<PathBuf>,
}
fn observe(
    gui: &mut GuiHandle,
    config_path: &Path,
    target: &FeatureTarget,
    edit: &str,
    revision: &str,
    check_available: bool,
    available: fn() -> bool,
) -> GuiResult<Observed> {
    let (project_name, feature_name) = resolve(gui, target)?;
    let (_, config, _) = crate::gui_settings::read_document(config_path)?;
    let app = gui.app_for_workflow();
    let pending = pending_for_feature(app, &project_name, &feature_name, 3)
        .into_iter()
        .find(|pending| pending.view.id == edit)
        .ok_or_else(|| {
            GuiError::conflict("This edit is no longer waiting. Refresh supervised edits.")
        })?;
    if pending.view.revision != revision
        || pending.view.answered
        || pending.view.unavailable.is_some()
    {
        return Err(GuiError::conflict(
            "This edit changed or the agent is no longer waiting. Refresh before explaining.",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted"))?;
    let project = &app.store.projects[pi];
    let feature = &project.features[fi];
    let input = pending.input;
    if Path::new(&input.cwd).canonicalize().ok() != feature.workdir.canonicalize().ok() {
        return Err(GuiError::conflict(
            "The waiting edit belongs to a different checkout",
        ));
    }
    let mut files = vec![input.file_path.clone()];
    files.extend(
        [&input.original_file, &input.proposed_file]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
    );
    let waiting: Vec<_> = [
        input.proceed_signal.as_deref(),
        input.response_file.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|path| Path::new(path).parent().map(Path::to_owned))
    .collect();
    let fs = filesystem(&feature.workdir, &files, &waiting, project.is_git)?;
    let sessions: Vec<_> = feature
        .sessions
        .iter()
        .map(|s| serde_json::json!([s.id, s.kind, s.tmux_window, s.claude_session_id, s.stopped]))
        .collect();
    let identity = hash(
        serde_json::to_vec(&serde_json::json!([
            project.id,
            project.name,
            project.repo,
            directory(&project.repo)?,
            feature.id,
            feature.name,
            feature.workdir,
            feature.branch,
            feature.tmux_session,
            feature.agent,
            feature.mode,
            feature.is_worktree,
            feature.status == crate::project::ProjectStatus::Stopped,
            sessions,
            fs,
            input.session_id,
            input.proceed_signal,
            input.response_file
        ]))
        .map_err(anyhow::Error::from)?,
    );
    let prepared = app.prepare_edit_explanation_with_config(
        &feature.workdir,
        &pending.view.path,
        input.old_snippet.as_deref().unwrap_or_default(),
        input.new_snippet.as_deref().unwrap_or_default(),
        &config,
    );
    match prepared {
        ExplanationPreparation::SavedNote(text) => {
            if text.len() > MAX_TEXT {
                return Err(GuiError::conflict(
                    "The saved review note exceeds the explanation display limit. Read the note file directly.",
                ));
            }
            Ok(Observed {
                source: None,
                note: Some(text),
                files,
                waiting,
            })
        }
        ExplanationPreparation::Call(call) => {
            let project_config = project_configuration(&project.repo)?;
            let claude = crate::project::AgentKind::Claude;
            if !allowed_explanation_agents(&project_config, &config.extension).contains(&claude) {
                return Err(GuiError::conflict(
                    "Claude is not allowed for this project. Saved review notes remain available.",
                ));
            }
            if check_available && !available() {
                return Err(GuiError::conflict(
                    "Claude is unavailable for explanations: no executable candidate. Saved review notes remain available.",
                ));
            }
            if call.prompt.len() > MAX_TEXT {
                return Err(GuiError::conflict(
                    "The explanation prompt exceeds the display limit",
                ));
            }
            Ok(Observed {
                source: Some(Source {
                    target: target.clone(),
                    edit: edit.to_owned(),
                    revision: revision.to_owned(),
                    workdir: feature.workdir.clone(),
                    identity,
                    filesystem: fs,
                    is_git: project.is_git,
                    call,
                }),
                note: None,
                files,
                waiting,
            })
        }
    }
}

fn require_admission(gui: &mut GuiHandle, config_path: &Path) -> GuiResult<()> {
    use crate::app::resource_gate::{
        StartPreconditions, describe_tripped, evaluate_start_preconditions,
    };
    let (_, config, _) = crate::gui_settings::read_document(config_path)?;
    let app = gui.app_for_workflow();
    let live = crate::resources::limits::LiveHarnesses::probe(app.tmux.as_ref());
    let count = crate::resources::limits::total_active_agents(&app.store, &live);
    if let StartPreconditions::NeedsConfirm {
        over_limit,
        low_memory,
    } = evaluate_start_preconditions(&config, count, crate::resources::mem::probe())
    {
        return Err(GuiError::needs_approval(format!(
            "Explaining this edit: {}. Continue anyway?",
            describe_tripped(over_limit, low_memory)
        )));
    }
    Ok(())
}

impl Controller {
    pub fn prepare(
        &mut self,
        gui: &mut GuiHandle,
        target: FeatureTarget,
        edit: &str,
        revision: &str,
    ) -> GuiResult<ExplanationView> {
        let observed = observe(
            gui,
            &self.config_path,
            &target,
            edit,
            revision,
            true,
            self.available,
        )?;
        self.job = None;
        self.preparation = None;
        if let Some(text) = observed.note {
            return Ok(ExplanationView::SavedNote { text });
        }
        let source = observed.source.expect("call or note");
        let owner = uuid::Uuid::new_v4().to_string();
        let view = ExplanationView::Precall {
            owner: owner.clone(),
            prompt: source.call.prompt.clone(),
            model: source.call.model.clone(),
            reasoning: source.call.reasoning,
        };
        self.preparation = Some(Preparation { owner, source });
        Ok(view)
    }

    pub fn start(
        &mut self,
        gui: &mut GuiHandle,
        owner: &str,
        approved: bool,
    ) -> GuiResult<ExplanationView> {
        let preparation = self
            .preparation
            .as_ref()
            .filter(|p| p.owner == owner)
            .ok_or_else(|| {
                GuiError::conflict("The explanation notice expired. Prepare it again.")
            })?;
        let old = &preparation.source;
        let observed = observe(
            gui,
            &self.config_path,
            &old.target,
            &old.edit,
            &old.revision,
            true,
            self.available,
        )?;
        if let Some(text) = observed.note {
            self.preparation = None;
            return Ok(ExplanationView::SavedNote { text });
        }
        let source = observed.source.expect("call or note");
        if source.identity != old.identity || source.call != old.call {
            self.preparation = None;
            return Err(GuiError::conflict(
                "The edit, checkout or explanation settings changed. Review a new notice.",
            ));
        }
        if !approved {
            require_admission(gui, &self.config_path)?;
        }
        self.preparation = None; // A notice is consumed once, before spawn.
        self.sequence += 1;
        let request = self.sequence;
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = cancelled.clone();
        let workdir = source.workdir.clone();
        let expected = source.filesystem.clone();
        let call = source.call.clone();
        let runner = self.runner;
        let files = observed.files;
        let waiting = observed.waiting;
        let is_git = source.is_git;
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("edit-explanation".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    anyhow::ensure!(!token.load(Ordering::Relaxed), "Explanation cancelled");
                    anyhow::ensure!(
                        filesystem(&workdir, &files, &waiting, is_git)
                            .map_err(|e| anyhow::anyhow!(e.message))?
                            == expected,
                        "The checkout or edit changed before the explanation started"
                    );
                    runner(&workdir, &call, &token)
                }))
                .map_err(|_| "The explanation worker stopped unexpectedly".to_owned())
                .and_then(|result| result.map_err(|e| format!("{e:#}")));
                let _ = sender.send(result);
            })
            .map_err(anyhow::Error::from)?;
        self.job = Some(Job {
            owner: owner.to_owned(),
            request,
            source,
            cancelled,
            receiver,
            finished: None,
        });
        Ok(ExplanationView::Running {
            owner: owner.to_owned(),
            request,
        })
    }

    pub fn poll(
        &mut self,
        gui: &mut GuiHandle,
        owner: &str,
        request: u64,
    ) -> GuiResult<ExplanationView> {
        let job = self
            .job
            .as_mut()
            .filter(|job| job.owner == owner && job.request == request)
            .ok_or_else(|| GuiError::conflict("This explanation is no longer active"))?;
        let checked = observe(
            gui,
            &self.config_path,
            &job.source.target,
            &job.source.edit,
            &job.source.revision,
            false,
            self.available,
        );
        if !checked.as_ref().is_ok_and(|current| {
            current.source.as_ref().is_some_and(|source| {
                source.identity == job.source.identity && source.call == job.source.call
            })
        }) {
            self.job = None;
            return Err(GuiError::conflict(
                "The waiting edit or checkout changed. The explanation was discarded.",
            ));
        }
        if job.finished.is_none() {
            match job.receiver.try_recv() {
                Ok(result) => job.finished = Some(result),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    job.finished = Some(Err("The explanation worker disconnected".into()))
                }
            }
        }
        Ok(match &job.finished {
            None => ExplanationView::Running {
                owner: owner.into(),
                request,
            },
            Some(Ok(text)) => ExplanationView::Complete {
                owner: owner.into(),
                request,
                text: text.clone(),
            },
            Some(Err(message)) => ExplanationView::Failed {
                owner: owner.into(),
                request,
                message: message.clone(),
            },
        })
    }

    pub fn cancel(&mut self, owner: &str) -> ExplanationView {
        if self.preparation.as_ref().is_some_and(|p| p.owner == owner) {
            self.preparation = None;
        }
        if self.job.as_ref().is_some_and(|job| job.owner == owner) {
            self.job = None;
        }
        ExplanationView::Cancelled {
            owner: owner.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_diff::{DiffContext, tests::fixture};
    use std::time::{Duration, Instant};

    fn fixture_edit() -> (
        tempfile::TempDir,
        GuiHandle,
        FeatureTarget,
        Controller,
        String,
        String,
    ) {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let waiting = dir.path().join("waiting");
        std::fs::create_dir(&waiting).unwrap();
        let notifications = repo.join(".claude/notifications");
        std::fs::create_dir_all(&notifications).unwrap();
        std::fs::write(waiting.join("old"), "old capture").unwrap();
        std::fs::write(waiting.join("new"), "new capture").unwrap();
        std::fs::write(notifications.join("edit.json"), serde_json::json!({
            "type": "diff-review", "session_id": "sess", "cwd": repo,
            "file_path": repo.join("code.txt"), "relative_path": "code.txt", "change_id": "change",
            "old_snippet": "old snippet", "new_snippet": "new snippet", "tool": "edit",
            "original_file": waiting.join("old"), "proposed_file": waiting.join("new"),
            "response_file": waiting.join("response"), "proceed_signal": waiting.join("proceed")
        }).to_string()).unwrap();
        let config = dir.path().join("config.json");
        std::fs::write(&config, r#"{"review_models":{"diff_explain":"configured-model"},"max_concurrent_agents":0,"low_memory_warn_mb":0}"#).unwrap();
        let mut tmux = crate::traits::MockTmuxOps::new();
        tmux.expect_list_panes().return_const(Vec::new());
        gui.app_for_workflow().tmux = Box::new(tmux);
        let controller = Controller {
            config_path: config,
            runner: fake_run,
            available: || true,
            ..Controller::default()
        };
        let edit = super::super::load(&mut gui, target.clone(), DiffContext::Standard)
            .unwrap()
            .edits
            .remove(0);
        (dir, gui, target, controller, edit.id, edit.revision)
    }
    fn fake_run(
        workdir: &Path,
        call: &ExplanationCall,
        cancelled: &AtomicBool,
    ) -> anyhow::Result<String> {
        std::fs::write(workdir.join("called"), &call.prompt)?;
        if workdir.join("hold").exists() {
            while !cancelled.load(Ordering::Relaxed) && !workdir.join("release").exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        anyhow::ensure!(!cancelled.load(Ordering::Relaxed), "cancelled");
        if workdir.join("panic").exists() {
            panic!("fixture panic");
        }
        Ok("An isolated explanation".into())
    }
    fn owner(view: ExplanationView) -> String {
        match view {
            ExplanationView::Precall { owner, .. } => owner,
            other => panic!("{other:?}"),
        }
    }
    fn wait_for(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(path.exists(), "{}", path.display());
    }
    fn external_change(dir: &Path, change: impl FnOnce(&mut crate::project::ProjectStore)) {
        let db = crate::db::AmfDb::open(&dir.join("amf.db")).unwrap();
        let (mut store, version) = db.load_store_versioned().unwrap();
        change(&mut store);
        db.save_store_checked(&store, version).unwrap();
    }

    #[test]
    fn note_and_precall_are_call_free_and_do_not_change_mode_or_answer() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let repo = dir.path().join("repo");
        let owner = owner(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
        );
        assert!(controller.job.is_none());
        controller.cancel(&owner);
        assert!(!repo.join("called").exists());
        std::fs::write(
            repo.join(".claude/review-notes.md"),
            "## code.txt\n\nSaved rationale.\n",
        )
        .unwrap();
        assert!(
            matches!(controller.prepare(&mut gui, target.clone(), &edit, &revision).unwrap(),
            ExplanationView::SavedNote { text } if text == "Saved rationale.")
        );
        assert!(matches!(
            gui.app_for_workflow().mode,
            crate::app::AppMode::Normal
        ));
        assert!(!dir.path().join("waiting/response").exists());
        assert!(!dir.path().join("waiting/proceed").exists());
        assert!(!repo.join("called").exists());
        std::fs::write(repo.join(".claude/review-notes.md"), "## code.txt\n\n").unwrap();
        assert!(
            matches!(controller.prepare(&mut gui, target.clone(), &edit, &revision).unwrap(),
            ExplanationView::SavedNote { text } if text.is_empty())
        );
        assert!(!repo.join("called").exists());
    }

    #[test]
    fn confirmation_is_one_shot_and_config_changes_require_a_new_notice() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let first = owner(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
        );
        std::fs::write(&controller.config_path, r#"{"review_model":"changed"}"#).unwrap();
        assert!(controller.start(&mut gui, &first, true).is_err());
        assert!(!dir.path().join("repo/called").exists());
        let next = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        let started = controller.start(&mut gui, &next, true).unwrap();
        assert!(controller.start(&mut gui, &next, true).is_err());
        let ExplanationView::Running { request, .. } = started else {
            panic!()
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match controller.poll(&mut gui, &next, request).unwrap() {
                ExplanationView::Complete { text, .. } => {
                    assert_eq!(text, "An isolated explanation");
                    break;
                }
                ExplanationView::Running { .. } => assert!(Instant::now() < deadline),
                other => panic!("{other:?}"),
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn sqlite_session_change_refuses_confirmation_and_preserves_sibling_edit() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        external_change(dir.path(), |store| {
            store.projects[0].features[0].tmux_session = "external-session".into();
        });
        assert!(controller.start(&mut gui, &prepared, true).is_err());
        assert_eq!(
            gui.app_for_workflow().store.projects[0].features[0].tmux_session,
            "external-session"
        );
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn non_git_preparation_succeeds_but_replaced_directory_refuses_confirmation() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        external_change(dir.path(), |store| store.projects[0].is_git = false);
        std::fs::remove_dir_all(dir.path().join("repo/.git")).unwrap();
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        std::fs::rename(dir.path().join("repo"), dir.path().join("old-repo")).unwrap();
        std::fs::create_dir(dir.path().join("repo")).unwrap();
        assert!(controller.start(&mut gui, &prepared, true).is_err());
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn branch_change_and_captured_file_change_refuse_previous_approval() {
        for mutate in [false, true] {
            let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
            let prepared = owner(
                controller
                    .prepare(&mut gui, target, &edit, &revision)
                    .unwrap(),
            );
            if mutate {
                std::fs::write(dir.path().join("waiting/new"), "changed captured bytes").unwrap();
            } else {
                assert!(
                    std::process::Command::new("git")
                        .arg("-C")
                        .arg(dir.path().join("repo"))
                        .args(["checkout", "-b", "external"])
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            }
            assert!(controller.start(&mut gui, &prepared, true).is_err());
        }
    }

    #[test]
    fn explanation_lifecycle_preserves_an_underlying_app_mode() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        gui.app_for_workflow().mode = crate::app::AppMode::NotificationPicker(7, None);
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        let ExplanationView::Running { request, .. } =
            controller.start(&mut gui, &prepared, true).unwrap()
        else {
            panic!()
        };
        wait_for(&dir.path().join("repo/called"));
        let _ = controller.poll(&mut gui, &prepared, request).unwrap();
        controller.cancel(&prepared);
        assert!(matches!(
            gui.app_for_workflow().mode,
            crate::app::AppMode::NotificationPicker(7, None)
        ));
        assert!(!dir.path().join("waiting/proceed").exists());
    }

    #[test]
    fn missing_claude_and_oversized_notes_do_not_launch_a_fallback() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let prepared = owner(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
        );
        controller.available = || false;
        assert!(controller.start(&mut gui, &prepared, true).is_err());
        assert!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap_err()
                .message
                .contains("unavailable")
        );
        let notes = dir.path().join("repo/.claude/review-notes.md");
        std::fs::write(&notes, "## code.txt\n\nSaved without Claude.\n").unwrap();
        assert!(matches!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
            ExplanationView::SavedNote { .. }
        ));
        std::fs::write(
            &notes,
            format!("## code.txt\n\n{}", "x".repeat(MAX_TEXT + 1)),
        )
        .unwrap();
        assert!(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap_err()
                .message
                .contains("display limit")
        );
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn malformed_or_unreadable_effective_project_config_never_enables_a_call() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let config = dir.path().join("repo/amf.json");
        for text in ["{broken", r#"{"allowed_agents":false}"#] {
            std::fs::write(&config, text).unwrap();
            assert!(
                controller
                    .prepare(&mut gui, target.clone(), &edit, &revision)
                    .is_err()
            );
            assert!(!dir.path().join("repo/called").exists());
        }
        std::fs::remove_file(&config).unwrap();
        std::fs::create_dir(&config).unwrap();
        assert!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .is_err()
        );
        std::fs::write(
            dir.path().join("repo/.claude/review-notes.md"),
            "## code.txt\n\nStill readable saved note.\n",
        )
        .unwrap();
        assert!(matches!(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
            ExplanationView::SavedNote { .. }
        ));
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn project_policy_uses_the_strict_snapshot_and_fresh_global_precedence() {
        use crate::project::AgentKind;
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let config = dir.path().join("repo/amf.json");
        std::fs::write(
            &controller.config_path,
            r#"{"extension":{"allowed_agents":["codex"]}}"#,
        )
        .unwrap();
        // The startup App configuration still permits Claude, but the current
        // global snapshot must refuse it when the project inherits choices.
        assert!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap_err()
                .message
                .contains("not allowed")
        );
        std::fs::write(&config, r#"{"allowed_agents":["claude","claude"]}"#).unwrap();
        assert!(matches!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
            ExplanationView::Precall { .. }
        ));
        let project = project_configuration(&dir.path().join("repo")).unwrap();
        let (_, global, _) = crate::gui_settings::read_document(&controller.config_path).unwrap();
        std::fs::write(&config, "{no permissive second read").unwrap();
        assert_eq!(
            allowed_explanation_agents(&project, &global.extension),
            vec![AgentKind::Claude]
        );
        assert!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .is_err()
        );
        // A present empty project list keeps shared normalization: allow all.
        std::fs::write(&config, r#"{"allowed_agents":[]}"#).unwrap();
        assert!(matches!(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
            ExplanationView::Precall { .. }
        ));
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn excluded_claude_never_previews_or_launches_but_saved_notes_still_work() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let config = dir.path().join("repo/amf.json");
        std::fs::write(config, r#"{"allowed_agents":["codex"]}"#).unwrap();
        assert!(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap_err()
                .message
                .contains("not allowed")
        );
        std::fs::write(
            dir.path().join("repo/.claude/review-notes.md"),
            "## code.txt\n\nSaved without a harness.\n",
        )
        .unwrap();
        assert!(matches!(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
            ExplanationView::SavedNote { .. }
        ));
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn newly_saved_note_replaces_the_pending_call_without_launching() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        std::fs::write(
            dir.path().join("repo/.claude/review-notes.md"),
            "## code.txt\n\nArrived while confirming.\n",
        )
        .unwrap();
        assert!(
            matches!(controller.start(&mut gui, &prepared, true).unwrap(),
            ExplanationView::SavedNote { text } if text == "Arrived while confirming.")
        );
        assert!(!dir.path().join("repo/called").exists());
    }

    #[test]
    fn malformed_config_refuses_call_and_resource_warning_keeps_notice_retryable() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let prepared = owner(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
        );
        std::fs::write(&controller.config_path, "{broken").unwrap();
        assert!(controller.start(&mut gui, &prepared, false).is_err());
        assert!(!dir.path().join("repo/called").exists());
        std::fs::write(
            &controller.config_path,
            r#"{"max_concurrent_agents":1,"low_memory_warn_mb":0}"#,
        )
        .unwrap();
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        let _lease = crate::resources::limits::HeadlessLease::acquire();
        let error = controller.start(&mut gui, &prepared, false).unwrap_err();
        assert!(matches!(
            error.kind,
            crate::gui_contract::GuiErrorKind::NeedsApproval
        ));
        assert!(!dir.path().join("repo/called").exists());
        assert!(matches!(
            controller.start(&mut gui, &prepared, true).unwrap(),
            ExplanationView::Running { .. }
        ));
    }

    #[test]
    fn worker_panic_is_a_bounded_error_and_external_deletion_cancels_only_its_owner() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        std::fs::write(dir.path().join("repo/panic"), "").unwrap();
        let prepared = owner(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
        );
        let ExplanationView::Running { request, .. } =
            controller.start(&mut gui, &prepared, true).unwrap()
        else {
            panic!()
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match controller.poll(&mut gui, &prepared, request).unwrap() {
                ExplanationView::Failed { message, .. } => {
                    assert!(message.contains("unexpectedly"));
                    break;
                }
                ExplanationView::Running { .. } => assert!(Instant::now() < deadline),
                other => panic!("{other:?}"),
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_file(dir.path().join("repo/panic")).unwrap();
        std::fs::write(dir.path().join("repo/hold"), "").unwrap();
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        let ExplanationView::Running { request, .. } =
            controller.start(&mut gui, &prepared, true).unwrap()
        else {
            panic!()
        };
        let token = controller.job.as_ref().unwrap().cancelled.clone();
        external_change(dir.path(), |store| store.projects[0].features.clear());
        assert!(controller.poll(&mut gui, &prepared, request).is_err());
        assert!(token.load(Ordering::Relaxed));
        assert!(gui.app_for_workflow().store.projects[0].features.is_empty());
    }

    #[test]
    fn external_feature_stop_cancels_running_explanation_without_changing_session_choices() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        external_change(dir.path(), |store| {
            store.projects[0].features[0].status = crate::project::ProjectStatus::Active
        });
        let prepared = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        std::fs::write(dir.path().join("repo/hold"), "").unwrap();
        let ExplanationView::Running { request, .. } =
            controller.start(&mut gui, &prepared, true).unwrap()
        else {
            panic!()
        };
        let token = controller.job.as_ref().unwrap().cancelled.clone();
        external_change(dir.path(), |store| {
            store.projects[0].features[0].status = crate::project::ProjectStatus::Stopped
        });
        assert!(controller.poll(&mut gui, &prepared, request).is_err());
        assert!(token.load(Ordering::Relaxed));
        assert!(!dir.path().join("waiting/proceed").exists());
    }

    #[test]
    fn answered_edit_discards_running_result_and_stale_cancel_does_not_cancel_replacement() {
        let (dir, mut gui, target, mut controller, edit, revision) = fixture_edit();
        let repo = dir.path().join("repo");
        std::fs::write(repo.join("hold"), "").unwrap();
        let first = owner(
            controller
                .prepare(&mut gui, target.clone(), &edit, &revision)
                .unwrap(),
        );
        let ExplanationView::Running {
            request: first_request,
            ..
        } = controller.start(&mut gui, &first, true).unwrap()
        else {
            panic!()
        };
        wait_for(&repo.join("called"));
        let first_token = controller.job.as_ref().unwrap().cancelled.clone();
        controller.cancel(&first);
        assert!(first_token.load(Ordering::Relaxed));
        let second = owner(
            controller
                .prepare(&mut gui, target, &edit, &revision)
                .unwrap(),
        );
        let ExplanationView::Running {
            request: second_request,
            ..
        } = controller.start(&mut gui, &second, true).unwrap()
        else {
            panic!()
        };
        let second_token = controller.job.as_ref().unwrap().cancelled.clone();
        controller.cancel(&first);
        assert!(!second_token.load(Ordering::Relaxed));
        assert!(controller.poll(&mut gui, &first, first_request).is_err());
        assert!(!second_token.load(Ordering::Relaxed));
        std::fs::write(dir.path().join("waiting/proceed"), "external answer").unwrap();
        assert!(controller.poll(&mut gui, &second, second_request).is_err());
        assert!(second_token.load(Ordering::Relaxed));
        assert!(!dir.path().join("waiting/response").exists());
    }
}
