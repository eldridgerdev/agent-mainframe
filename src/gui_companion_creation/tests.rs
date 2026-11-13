//! Companion creation: private git repositories plus a bare "remote" on disk,
//! mocked tmux (any launch call would panic the mock, so every test also proves
//! no agent is started) and no GitHub access at all.
use super::*;
use crate::github::PrRef;
use crate::gui_contract::GuiErrorKind;
use crate::project::{ProjectStatus, ReviewSource, SessionKind, TriageSource};
use crate::traits::WorktreeOps;
use std::sync::mpsc;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Where the PR head comes from.
#[derive(Clone, Copy, PartialEq)]
enum Head {
    /// The local `feature` tip, which `origin` also has.
    Local,
    /// A commit only `origin` has: creation must fetch it.
    OnlyOnOrigin,
    /// A well-formed id nobody has (a fork's commit): the fetch must fail.
    Nowhere,
    /// The local tip, but the repository has no `origin` at all.
    NoRemote,
}

struct Fx {
    dir: tempfile::TempDir,
    gui: Arc<Mutex<GuiHandle>>,
    workflow: PrDestinationWorkflow,
    identity: PrIdentity,
    repo: PathBuf,
    head: String,
    source_id: String,
}

fn fixture(head_mode: Head) -> Fx {
    let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
    let repo = dir.path().join("repo");
    let remote = dir.path().join("remote.git");
    let local_tip = run_git(&repo, &["rev-parse", "HEAD"]);
    let mut head_ref = "feature".to_string();
    let mut head = local_tip.clone();
    if head_mode != Head::NoRemote {
        std::fs::create_dir(&remote).unwrap();
        run_git(&remote, &["init", "--bare", "-b", "main"]);
        run_git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&repo, &["push", "origin", "feature", "main"]);
    }
    match head_mode {
        Head::OnlyOnOrigin => {
            let other = dir.path().join("other");
            run_git(
                dir.path(),
                &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
            );
            run_git(&other, &["checkout", "-b", "pr-branch", "origin/feature"]);
            std::fs::write(other.join("pr.txt"), "from the PR").unwrap();
            run_git(&other, &["add", "."]);
            run_git(&other, &["commit", "-m", "pr work"]);
            run_git(&other, &["push", "origin", "pr-branch"]);
            head = run_git(&other, &["rev-parse", "HEAD"]);
            head_ref = "pr-branch".into();
        }
        Head::Nowhere => {
            head = "1234567890abcdef1234567890abcdef12345678".into();
            head_ref = "fork-branch".into();
        }
        Head::Local | Head::NoRemote => {}
    }
    let app = gui.app_for_workflow();
    app.config.low_memory_warn_mb = 0;
    app.config.max_concurrent_agents = 0;
    app.worktree = Arc::new(crate::worktree::WorktreeManager);
    let source_id = target.feature_id.clone();
    let id = crate::gui_pr_workspace::begin(&mut gui, target).unwrap();
    gui.pr_workspace_context.as_mut().unwrap().pr = Some(PrRef {
        number: 42,
        head_sha: head.clone(),
        url: "https://example.test/pull/42".into(),
        owner: "fixture".into(),
        repo: "repo".into(),
        head_ref,
    });
    let workflow = PrDestinationWorkflow::Workspace { workflow_id: id };
    let identity = catalog(&mut gui, &workflow).unwrap().identity;
    Fx {
        dir,
        gui: Arc::new(Mutex::new(gui)),
        workflow,
        identity,
        repo,
        head,
        source_id,
    }
}

fn settings(branch: &str) -> CompanionSettings {
    CompanionSettings {
        branch: branch.into(),
        agent: AgentKind::Claude,
        mode: VibeMode::Vibe,
        review: false,
        design_review: false,
        enable_chrome: false,
    }
}

fn choice(branch: &str, hook_choice: Option<&str>) -> PrDestinationChoice {
    PrDestinationChoice::CompanionFeature {
        settings: settings(branch),
        hook_choice: hook_choice.map(Into::into),
    }
}

impl Fx {
    fn with<T>(&self, f: impl FnOnce(&mut GuiHandle) -> T) -> T {
        f(&mut self.gui.lock().unwrap())
    }

    fn request(&self, choice: PrDestinationChoice) -> PrDestinationRequest {
        PrDestinationRequest {
            workflow: self.workflow.clone(),
            identity: self.identity.clone(),
            choice,
        }
    }

    fn try_preview(&self, choice: PrDestinationChoice) -> GuiResult<PrDestinationPreview> {
        let request = self.request(choice);
        let read = self.with(|gui| preview_read(gui, &request))?;
        Ok(read.run())
    }

    fn preview(&self, choice: PrDestinationChoice) -> PrDestinationPreview {
        self.try_preview(choice).unwrap()
    }

    fn prepare_request(
        &self,
        choice: PrDestinationChoice,
        revision: &str,
        approved: bool,
    ) -> PrDestinationPrepareRequest {
        PrDestinationPrepareRequest {
            workflow: self.workflow.clone(),
            identity: self.identity.clone(),
            choice,
            preview_revision: revision.into(),
            approved,
        }
    }

    fn locker(&self) -> Arc<dyn GuiLocker> {
        self.gui.clone()
    }

    fn start_with(
        &self,
        choice: PrDestinationChoice,
        revision: &str,
        approved: bool,
        fetcher: Fetcher,
    ) -> GuiResult<PrDestinationPrepared> {
        start_with(
            self.locker(),
            self.prepare_request(choice, revision, approved),
            fetcher,
            Arc::new(|| {}),
        )
    }

    /// Preview, then start with the real fetcher; returns the operation id.
    fn begin(&self, branch: &str) -> String {
        let choice = choice(branch, None);
        let shown = self.preview(choice.clone());
        let started = self
            .start_with(choice, &shown.revision, false, git_fetcher())
            .unwrap();
        assert_eq!(started.status, PrepareStatus::Started);
        started.operation_id
    }

    fn create(&self, branch: &str) -> PrDestinationOperation {
        wait_done(&self.begin(branch))
    }

    fn features(&self) -> Vec<Feature> {
        self.with(|gui| gui.app_for_workflow().store.projects[0].features.clone())
    }

    fn companion(&self, branch: &str) -> Feature {
        self.features()
            .into_iter()
            .find(|f| f.name == branch)
            .unwrap_or_else(|| panic!("no feature {branch}"))
    }

    fn worktree_dir(&self, branch: &str) -> PathBuf {
        self.repo
            .join(".worktrees")
            .join(crate::project::worktree_name("Demo", branch))
    }

    fn local_branch_exists(&self, branch: &str) -> bool {
        crate::app::triage_feature::branch_exists(&self.repo, branch)
    }

    fn set_worktree(&self, ops: impl WorktreeOps + 'static) {
        self.with(|gui| gui.app_for_workflow().worktree = Arc::new(ops));
    }

    fn set_hook(&self, hook: crate::extension::HookConfig) {
        self.with(|gui| {
            gui.app_for_workflow()
                .config
                .extension
                .lifecycle_hooks
                .on_worktree_created = Some(hook);
        });
    }
}

fn wait_for(
    operation_id: &str,
    done: impl Fn(&PrDestinationOperation) -> bool,
) -> PrDestinationOperation {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let view = status(operation_id).unwrap();
        if done(&view) {
            return view;
        }
        assert!(Instant::now() < deadline, "timed out: {view:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_done(operation_id: &str) -> PrDestinationOperation {
    wait_for(operation_id, |view| view.state != OperationState::Running)
}

/// Wraps the real worktree manager with hooks around `create_from`.
struct Wrapped {
    after_create: Box<dyn Fn(&Path) + Send + Sync>,
}

impl WorktreeOps for Wrapped {
    fn remove(&self, repo: &Path, path: &Path) -> anyhow::Result<()> {
        crate::worktree::WorktreeManager.remove(repo, path)
    }
    fn repo_root(&self, path: &Path) -> anyhow::Result<PathBuf> {
        crate::worktree::WorktreeManager.repo_root(path)
    }
    fn create(&self, repo: &Path, name: &str, branch: &str) -> anyhow::Result<PathBuf> {
        crate::worktree::WorktreeManager.create(repo, name, branch)
    }
    fn create_from(
        &self,
        repo: &Path,
        name: &str,
        branch: &str,
        base: &str,
    ) -> anyhow::Result<PathBuf> {
        let path = crate::worktree::WorktreeManager.create_from(repo, name, branch, base)?;
        (self.after_create)(&path);
        Ok(path)
    }
    fn create_fork_base(
        &self,
        repo: &Path,
        name: &str,
        branch: &str,
        commit: &str,
    ) -> anyhow::Result<PathBuf> {
        crate::worktree::WorktreeManager.create_fork_base(repo, name, branch, commit)
    }
}

/// A fetcher that blocks until released or cancelled, like a slow remote.
fn slow_fetcher(release: mpsc::Receiver<()>) -> (Fetcher, Arc<AtomicBool>) {
    let entered = Arc::new(AtomicBool::new(false));
    let release = Mutex::new(release);
    let flag = entered.clone();
    let fetcher: Fetcher = Arc::new(move |workdir, sha, cancel| {
        flag.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            if release.lock().unwrap().try_recv().is_ok() {
                // Released: do the real fetch so a later step can proceed.
                return fetch_head(workdir, sha, cancel);
            }
            assert!(Instant::now() < deadline, "fetcher never released");
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    (fetcher, entered)
}

fn wait_flag(flag: &AtomicBool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !flag.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------- preview

#[test]
fn catalog_lists_harnesses_modes_a_suggested_name_and_the_pr_branch() {
    let fx = fixture(Head::Local);
    let view = fx.with(|gui| catalog(gui, &fx.workflow)).unwrap();
    assert_eq!(view.suggested_branch, "feature-triage");
    assert_eq!(view.pr_branch, "feature");
    assert_eq!(view.source_name, "Feature");
    assert!(view.agents.iter().any(|a| a.agent == AgentKind::Claude));
    let vibeless = view
        .modes
        .iter()
        .find(|m| m.mode == VibeMode::Vibeless)
        .unwrap();
    assert_eq!(vibeless.unsupported_agents, vec![AgentKind::Codex]);
    assert!(view.hook.is_none());
    // The suggestion skips names a feature, a branch or a path already uses.
    run_git(&fx.repo, &["branch", "feature-triage"]);
    let view = fx.with(|gui| catalog(gui, &fx.workflow)).unwrap();
    assert_eq!(view.suggested_branch, "feature-triage-2");
}

#[test]
fn preview_names_branch_base_path_and_pr_branch_and_changes_nothing() {
    let fx = fixture(Head::Local);
    let shown = fx.preview(choice("  fix-triage ", None));
    let c = shown.companion.as_ref().unwrap();
    assert_eq!(c.branch, "fix-triage");
    assert_eq!(c.base_sha, fx.head);
    assert_eq!(c.base_short, fx.head[..7]);
    assert_eq!(c.pr_branch, "feature");
    assert_eq!(c.pr_branch_on_origin, Some(true));
    assert!(c.pr_branch_note.is_none());
    assert!(!c.will_fetch);
    assert!(c.hook.is_none());
    assert_eq!(
        c.destination_path,
        fx.worktree_dir("fix-triage").display().to_string()
    );
    assert_eq!(
        (c.agent.clone(), c.mode.clone()),
        (AgentKind::Claude, VibeMode::Vibe)
    );
    assert!(shown.stopped && shown.creates_session && shown.resource_warning.is_none());
    let text = shown.consequences.join(" ");
    for needle in [
        "new branch 'fix-triage'",
        "The agent is stopped",
        "Nothing is pushed and nothing is posted to GitHub",
        "(on the remote)",
        "kept and its path is shown",
    ] {
        assert!(text.contains(needle), "{needle}\n{text}");
    }
    // Nothing was created or registered, and the preview is stable.
    assert_eq!(fx.features().len(), 1);
    assert!(!fx.worktree_dir("fix-triage").exists());
    assert!(!fx.local_branch_exists("fix-triage"));
    assert_eq!(
        fx.preview(choice("fix-triage", None)).revision,
        shown.revision
    );
}

#[test]
fn the_remote_probe_is_not_part_of_the_revision_but_a_missing_pr_branch_is_flagged() {
    let fx = fixture(Head::Nowhere);
    let shown = fx.preview(choice("from-fork", None));
    let c = shown.companion.as_ref().unwrap();
    assert_eq!(c.pr_branch, "fork-branch");
    assert_eq!(c.pr_branch_on_origin, Some(false));
    let note = c.pr_branch_note.as_deref().unwrap();
    assert!(
        note.contains("fork") && note.contains("Integrate"),
        "{note}"
    );
    assert!(c.will_fetch, "an unknown head must be fetched first");
    // Creating is still allowed to be attempted: the preview does not refuse.
    assert!(!shown.revision.is_empty());
}

#[test]
fn a_remote_that_cannot_be_read_is_unknown_not_missing() {
    let fx = fixture(Head::NoRemote);
    let shown = fx.preview(choice("offline", None));
    let c = shown.companion.as_ref().unwrap();
    assert_eq!(c.pr_branch_on_origin, None);
    assert!(
        c.pr_branch_note
            .as_deref()
            .unwrap()
            .contains("Could not check")
    );
}

#[test]
fn a_pr_branch_that_moved_on_the_remote_is_noted() {
    let fx = fixture(Head::Local);
    let other = fx.dir.path().join("mover");
    run_git(
        fx.dir.path(),
        &[
            "clone",
            fx.dir.path().join("remote.git").to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    run_git(&other, &["checkout", "feature"]);
    std::fs::write(other.join("later.txt"), "x").unwrap();
    run_git(&other, &["add", "."]);
    run_git(&other, &["commit", "-m", "later"]);
    run_git(&other, &["push", "origin", "feature"]);
    let shown = fx.preview(choice("moved", None));
    let c = shown.companion.unwrap();
    assert_eq!(c.pr_branch_on_origin, Some(true));
    assert!(c.pr_branch_note.unwrap().contains("moved past"));
}

#[test]
fn the_head_is_seeded_from_even_when_the_local_pr_branch_is_ahead_of_it() {
    // The TUI would branch from the local `feature` tip here. The GUI hands a fix
    // only to a companion whose base IS the PR head, so it seeds from the head.
    let fx = fixture(Head::Local);
    std::fs::write(fx.repo.join("unpushed.txt"), "x").unwrap();
    run_git(&fx.repo, &["add", "."]);
    run_git(&fx.repo, &["commit", "-m", "unpushed"]);
    let tip = run_git(&fx.repo, &["rev-parse", "HEAD"]);
    assert_ne!(tip, fx.head);
    let shown = fx.preview(choice("seeded", None));
    assert_eq!(shown.companion.unwrap().base_sha, fx.head);
    let done = fx.create("seeded");
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    let feature = fx.companion("seeded");
    assert_eq!(feature.triage_source.unwrap().base_sha, fx.head);
    assert_eq!(
        run_git(&fx.worktree_dir("seeded"), &["rev-parse", "HEAD"]),
        fx.head
    );
}

#[test]
fn refusals_name_the_collision_and_never_touch_an_existing_branch() {
    let fx = fixture(Head::Local);
    // A feature with the (normalised) name.
    fx.with(|gui| {
        let app = gui.app_for_workflow();
        let mut twin = app.store.projects[0].features[0].clone();
        twin.id = "twin".into();
        twin.name = "Taken-Name".into();
        twin.tmux_session = "amf-twin".into();
        app.store.projects[0].features.push(twin);
    });
    let error = fx.try_preview(choice("taken-name", None)).unwrap_err();
    assert_eq!(error.kind, GuiErrorKind::Conflict);
    assert!(
        error.message.contains("already exists"),
        "{}",
        error.message
    );
    // A local branch.
    run_git(&fx.repo, &["branch", "old-work"]);
    let before = run_git(&fx.repo, &["rev-parse", "old-work"]);
    let error = fx.try_preview(choice("old-work", None)).unwrap_err();
    assert!(error.message.contains("local branch"), "{}", error.message);
    assert!(error.message.contains("never resets"), "{}", error.message);
    assert_eq!(run_git(&fx.repo, &["rev-parse", "old-work"]), before);
    // A checkout path.
    std::fs::create_dir_all(fx.worktree_dir("path-taken")).unwrap();
    let error = fx.try_preview(choice("path-taken", None)).unwrap_err();
    assert!(
        error.message.contains("already exists"),
        "{}",
        error.message
    );
    // Names Git would not accept, and an empty one.
    for bad in ["", "   ", "bad name", "a..b", "-leading", "x~1"] {
        let error = fx.try_preview(choice(bad, None)).unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::Conflict, "{bad:?}");
    }
    assert_eq!(fx.features().len(), 2);
}

#[test]
fn a_companion_for_this_head_is_refused_but_one_for_an_older_head_is_not() {
    let fx = fixture(Head::Local);
    let done = fx.create("first");
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    let error = fx.try_preview(choice("second", None)).unwrap_err();
    assert!(
        error.message.contains("already exists: 'first'"),
        "{}",
        error.message
    );
    // Make the existing one look older.
    fx.with(|gui| {
        let app = gui.app_for_workflow();
        for feature in &mut app.store.projects[0].features {
            if let Some(link) = feature.triage_source.as_mut() {
                link.base_sha = "f".repeat(40);
            }
        }
    });
    let shown = fx.preview(choice("second", None));
    assert_eq!(
        shown.companion.unwrap().older_companions,
        vec!["first".to_string()]
    );
}

#[test]
fn a_source_feature_with_both_links_is_refused() {
    let fx = fixture(Head::Local);
    fx.with(|gui| {
        let app = gui.app_for_workflow();
        let source = &mut app.store.projects[0].features[0];
        source.triage_source = Some(TriageSource {
            pr_number: 1,
            source_feature_id: "x".into(),
            pr_branch: "b".into(),
            base_sha: "a".repeat(40),
        });
        source.review_source = Some(ReviewSource {
            source_feature_id: "y".into(),
            target_branch: "b".into(),
            base_sha: "a".repeat(40),
        });
    });
    let error = fx.try_preview(choice("both", None)).unwrap_err();
    assert!(error.message.contains("both"), "{}", error.message);
}

#[test]
fn an_unavailable_or_unsupported_harness_is_refused() {
    let fx = fixture(Head::Local);
    let codex_vibeless = PrDestinationChoice::CompanionFeature {
        settings: CompanionSettings {
            agent: AgentKind::Codex,
            mode: VibeMode::Vibeless,
            ..settings("codex-triage")
        },
        hook_choice: None,
    };
    let error = fx.try_preview(codex_vibeless).unwrap_err();
    assert!(error.message.contains("Vibeless"), "{}", error.message);
    let mut tmux = crate::traits::MockTmuxOps::new();
    tmux.expect_check_harness_available()
        .returning(|_| anyhow::bail!("claude is not installed"));
    fx.with(|gui| gui.app_for_workflow().tmux = Box::new(tmux));
    let error = fx.try_preview(choice("no-claude", None)).unwrap_err();
    assert!(error.message.contains("not installed"), "{}", error.message);
}

// ----------------------------------------------------------------- create

#[test]
fn creation_registers_a_stopped_linked_companion_and_starts_nothing() {
    let fx = fixture(Head::Local);
    let started = fx.begin("fix-triage");
    let done = wait_done(&started);
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    assert_eq!(done.phase, OperationPhase::Done);
    assert!(done.stopped && done.message.contains("stopped"));
    assert_eq!(done.destination_kind, DestinationKind::Companion);
    let feature = fx.companion("fix-triage");
    assert_eq!(feature.status, ProjectStatus::Stopped);
    assert_eq!(feature.branch, "fix-triage");
    assert_eq!(feature.mode, VibeMode::Vibe);
    assert!(!feature.plan_mode);
    assert!(feature.review_source.is_none(), "exactly one link");
    let link = feature.triage_source.clone().unwrap();
    assert_eq!(
        (
            link.pr_number,
            link.source_feature_id.as_str(),
            link.pr_branch.as_str(),
            link.base_sha.as_str()
        ),
        (42, fx.source_id.as_str(), "feature", fx.head.as_str())
    );
    assert_eq!(feature.sessions.len(), 1);
    let session = &feature.sessions[0];
    assert_eq!(session.label, crate::app::pr_review::TRIAGE_SESSION_LABEL);
    assert_eq!(session.kind, SessionKind::Claude);
    let target = done.created_target.clone().unwrap();
    assert_eq!(target.feature_id, feature.id);
    assert_eq!(target.session_id, session.id);
    // On disk: the worktree is on the new branch at the PR head.
    let dir = fx.worktree_dir("fix-triage");
    assert_eq!(done.retained_path.as_deref(), dir.to_str());
    assert_eq!(run_git(&dir, &["rev-parse", "HEAD"]), fx.head);
    assert_eq!(
        run_git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "fix-triage"
    );
    // Persisted, not only in memory.
    let stored = fx.with(|gui| {
        gui.app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .load_store()
            .unwrap()
    });
    assert!(
        stored.projects[0]
            .features
            .iter()
            .any(|f| f.name == "fix-triage")
    );
    // The returned target and revision are exactly what the handoff validates.
    let PrDestinationWorkflow::Workspace { workflow_id } = &fx.workflow else {
        unreachable!()
    };
    let rows = fx
        .with(|gui| crate::gui_pr_workspace::fix_targets(gui, workflow_id))
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.target.session_id == target.session_id)
        .expect("the companion session is a fix target");
    assert_eq!(row.destination_kind, DestinationKind::Companion);
    assert!(row.stopped);
    assert_eq!(
        Some(row.destination_revision.clone()),
        done.destination_revision
    );
}

#[test]
fn a_missing_head_is_fetched_then_used_and_the_revision_does_not_change() {
    let fx = fixture(Head::OnlyOnOrigin);
    assert!(!crate::app::triage_feature::commit_exists(
        &fx.repo, &fx.head
    ));
    let choice = choice("fetched", None);
    let shown = fx.preview(choice.clone());
    assert!(shown.companion.as_ref().unwrap().will_fetch);
    assert!(
        shown
            .consequences
            .join(" ")
            .contains("fetches it from the remote")
    );
    // Previewing fetched nothing.
    assert!(!crate::app::triage_feature::commit_exists(
        &fx.repo, &fx.head
    ));
    let started = fx
        .start_with(choice, &shown.revision, false, git_fetcher())
        .unwrap();
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    assert_eq!(
        run_git(&fx.worktree_dir("fetched"), &["rev-parse", "HEAD"]),
        fx.head
    );
    assert_eq!(
        fx.companion("fetched").triage_source.unwrap().base_sha,
        fx.head
    );
    assert!(crate::app::triage_feature::commit_exists(
        &fx.repo, &fx.head
    ));
}

#[test]
fn a_head_that_cannot_be_fetched_fails_closed_and_creates_nothing() {
    let fx = fixture(Head::Nowhere);
    let done = fx.create("never");
    assert_eq!(done.state, OperationState::Failed);
    assert!(done.message.contains("Could not fetch"), "{}", done.message);
    assert!(
        done.message.contains("Nothing was created"),
        "{}",
        done.message
    );
    assert!(done.retained_path.is_none() && done.created_target.is_none());
    assert_eq!(fx.features().len(), 1);
    assert!(!fx.worktree_dir("never").exists());
    assert!(!fx.local_branch_exists("never"));
    // The workflow's slot is free again.
    assert!(fx.try_preview(choice("never", None)).is_ok());
}

#[test]
fn a_fetch_that_succeeds_without_the_commit_still_fails_closed() {
    let fx = fixture(Head::Nowhere);
    let fetcher: Fetcher = Arc::new(|_, _, _| Ok(()));
    let choice = choice("liar", None);
    let shown = fx.preview(choice.clone());
    let started = fx
        .start_with(choice, &shown.revision, false, fetcher)
        .unwrap();
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Failed);
    assert!(
        done.message.contains("still not in this repository"),
        "{}",
        done.message
    );
    assert_eq!(fx.features().len(), 1);
    assert!(!fx.worktree_dir("liar").exists());
}

#[test]
fn start_returns_while_the_work_runs_and_a_duplicate_submit_gets_the_same_operation() {
    let fx = fixture(Head::OnlyOnOrigin);
    let (release, rx) = mpsc::channel();
    let (fetcher, entered) = slow_fetcher(rx);
    let choice = choice("slow", None);
    let shown = fx.preview(choice.clone());
    let time = Instant::now();
    let started = fx
        .start_with(choice.clone(), &shown.revision, false, fetcher.clone())
        .unwrap();
    assert!(time.elapsed() < Duration::from_secs(5));
    assert_eq!(started.status, PrepareStatus::Started);
    wait_flag(&entered);
    let running = status(&started.operation_id).unwrap();
    assert_eq!(running.state, OperationState::Running);
    assert_eq!(running.phase, OperationPhase::Fetching);
    // The GUI lock is free while the fetch runs.
    assert!(fx.gui.try_lock().is_ok());
    // Double click / second submit: same operation, no second worker.
    let again = fx
        .start_with(choice, &shown.revision, false, fetcher)
        .unwrap();
    assert_eq!(again.status, PrepareStatus::InProgress);
    assert_eq!(again.operation_id, started.operation_id);
    release.send(()).unwrap();
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    assert_eq!(fx.features().iter().filter(|f| f.name == "slow").count(), 1);
}

// ----------------------------------------------------------------- cancel

#[test]
fn cancel_while_fetching_stops_the_fetch_and_creates_nothing() {
    let fx = fixture(Head::OnlyOnOrigin);
    let (_release, rx) = mpsc::channel();
    let (fetcher, entered) = slow_fetcher(rx);
    let choice = choice("cancelled", None);
    let shown = fx.preview(choice.clone());
    let started = fx
        .start_with(choice, &shown.revision, false, fetcher)
        .unwrap();
    wait_flag(&entered);
    let view = cancel(&started.operation_id).unwrap();
    assert!(view.cancel_requested);
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Cancelled);
    assert!(
        done.message.contains("Nothing was created"),
        "{}",
        done.message
    );
    assert_eq!(fx.features().len(), 1);
    assert!(!fx.worktree_dir("cancelled").exists());
    assert!(!fx.local_branch_exists("cancelled"));
    // Idempotent once finished, and unknown ids are not found.
    assert_eq!(
        cancel(&started.operation_id).unwrap().state,
        OperationState::Cancelled
    );
    assert_eq!(
        cancel("no-such-operation").unwrap_err().kind,
        GuiErrorKind::NotFound
    );
    assert_eq!(
        status("no-such-operation").unwrap_err().kind,
        GuiErrorKind::NotFound
    );
}

#[test]
fn a_fetch_that_finishes_after_a_cancel_request_still_creates_nothing() {
    let fx = fixture(Head::OnlyOnOrigin);
    let entered = Arc::new(AtomicBool::new(false));
    let proceed = Arc::new(AtomicBool::new(false));
    let (e, p) = (entered.clone(), proceed.clone());
    let fetcher: Fetcher = Arc::new(move |workdir, sha, _cancel| {
        e.store(true, Ordering::SeqCst);
        while !p.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Deliberately ignores the cancel flag, like a fetch that was already done.
        fetch_head(workdir, sha, &AtomicBool::new(false))
    });
    let choice = choice("late-cancel", None);
    let shown = fx.preview(choice.clone());
    let started = fx
        .start_with(choice, &shown.revision, false, fetcher)
        .unwrap();
    wait_flag(&entered);
    cancel(&started.operation_id).unwrap();
    proceed.store(true, Ordering::SeqCst);
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Cancelled);
    assert!(!fx.worktree_dir("late-cancel").exists());
    assert_eq!(fx.features().len(), 1);
}

#[test]
fn cancel_after_the_worktree_exists_keeps_and_registers_it_and_skips_the_hook() {
    let fx = fixture(Head::Local);
    let marker = fx.dir.path().join("hook-ran");
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "touch '{}'",
        marker.display()
    )));
    let (reached_tx, reached_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let reached_tx = Mutex::new(reached_tx);
    fx.set_worktree(Wrapped {
        after_create: Box::new(move |_| {
            reached_tx.lock().unwrap().send(()).unwrap();
            go_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(20))
                .unwrap();
        }),
    });
    let id = fx.begin("half-way");
    reached_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(
        fx.worktree_dir("half-way").exists(),
        "the checkout exists now"
    );
    cancel(&id).unwrap();
    go_tx.send(()).unwrap();
    let done = wait_done(&id);
    assert_eq!(done.state, OperationState::Cancelled, "{}", done.message);
    assert!(done.message.contains("was kept at"), "{}", done.message);
    assert!(
        done.message.contains("no agent was started"),
        "{}",
        done.message
    );
    assert!(!marker.exists(), "the hook must not run after a cancel");
    // Kept AND registered (stopped), so it stays visible and recoverable.
    let feature = fx.companion("half-way");
    assert_eq!(feature.status, ProjectStatus::Stopped);
    assert!(feature.triage_source.is_some());
    assert_eq!(done.created_target.unwrap().feature_id, feature.id);
}

#[test]
fn cancel_during_the_hook_stops_its_process_tree_and_keeps_the_companion() {
    let fx = fixture(Head::Local);
    let started_marker = fx.dir.path().join("hook-started");
    let finished_marker = fx.dir.path().join("hook-finished");
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "touch '{}'; sleep 30; touch '{}'",
        started_marker.display(),
        finished_marker.display()
    )));
    let id = fx.begin("hooked-cancel");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !started_marker.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(status(&id).unwrap().phase, OperationPhase::RunningHook);
    cancel(&id).unwrap();
    let done = wait_done(&id);
    assert_eq!(done.state, OperationState::Cancelled, "{}", done.message);
    assert!(!finished_marker.exists());
    assert_eq!(fx.companion("hooked-cancel").status, ProjectStatus::Stopped);
}

// ------------------------------------------------------------------ hooks

#[test]
fn the_hook_runs_once_in_the_new_checkout_with_the_previewed_choice() {
    let fx = fixture(Head::Local);
    fx.set_hook(crate::extension::HookConfig::WithPrompt {
        script: "printf '%s' \"$AMF_HOOK_CHOICE\" > hook-output.txt".into(),
        prompt: crate::extension::HookPrompt {
            title: "Setup".into(),
            options: vec!["offline".into(), "full".into()],
        },
    });
    let pending = fx.preview(choice("hooked", None));
    let hook = pending.companion.unwrap().hook.unwrap();
    assert!(hook.choice_required && hook.choice.is_none());
    assert_eq!(hook.options, vec!["offline", "full"]);
    // Unanswered, invalid and answered.
    let unanswered = fx.start_with(
        choice("hooked", None),
        &pending.revision,
        false,
        git_fetcher(),
    );
    assert!(
        unanswered
            .unwrap_err()
            .message
            .contains("requires a choice")
    );
    assert_eq!(
        fx.try_preview(choice("hooked", Some("nope")))
            .unwrap_err()
            .kind,
        GuiErrorKind::Conflict
    );
    let answered = choice("hooked", Some("full"));
    let shown = fx.preview(answered.clone());
    let shown_hook = shown.companion.unwrap().hook.unwrap();
    assert!(!shown_hook.choice_required);
    assert!(shown.consequences.join(" ").contains("Setup: full"));
    let started = fx
        .start_with(answered, &shown.revision, false, git_fetcher())
        .unwrap();
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    assert!(done.hook_warning.is_none());
    assert_eq!(
        std::fs::read_to_string(fx.worktree_dir("hooked").join("hook-output.txt")).unwrap(),
        "full"
    );
}

#[test]
fn a_hook_changed_after_the_preview_makes_it_stale_and_runs_nothing() {
    let fx = fixture(Head::Local);
    let marker = fx.dir.path().join("ran");
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "touch '{}'",
        marker.display()
    )));
    let choice = choice("stale-hook", None);
    let shown = fx.preview(choice.clone());
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "touch '{}'; echo changed",
        marker.display()
    )));
    let error = fx
        .start_with(choice, &shown.revision, false, git_fetcher())
        .unwrap_err();
    assert_eq!(error.kind, GuiErrorKind::Conflict);
    assert!(
        error.message.contains("changed since the preview"),
        "{}",
        error.message
    );
    assert!(!marker.exists());
    assert_eq!(fx.features().len(), 1);
    assert!(!fx.worktree_dir("stale-hook").exists());
}

#[test]
fn a_failing_hook_keeps_the_companion_with_no_agent() {
    let fx = fixture(Head::Local);
    fx.set_hook(crate::extension::HookConfig::Script(
        "echo broke >&2; exit 4".into(),
    ));
    let done = fx.create("broken-hook");
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    assert!(done.hook_warning.as_deref().unwrap().contains("broke"));
    assert!(
        done.message.contains("setup script failed"),
        "{}",
        done.message
    );
    assert!(
        done.message.contains("no agent was started"),
        "{}",
        done.message
    );
    let feature = fx.companion("broken-hook");
    assert_eq!(feature.status, ProjectStatus::Stopped);
    assert!(feature.sessions[0].stopped || feature.status == ProjectStatus::Stopped);
    assert!(fx.worktree_dir("broken-hook").exists());
}

// ------------------------------------------------------------ stale / gate

#[test]
fn any_change_since_the_preview_is_a_stale_conflict() {
    let fx = fixture(Head::Local);
    let shown = fx.preview(choice("one", None));
    // Different settings.
    let other = PrDestinationChoice::CompanionFeature {
        settings: CompanionSettings {
            mode: VibeMode::SuperVibe,
            ..settings("one")
        },
        hook_choice: None,
    };
    let error = fx
        .start_with(other, &shown.revision, false, git_fetcher())
        .unwrap_err();
    assert!(
        error.message.contains("changed since the preview"),
        "{}",
        error.message
    );
    // A different branch name.
    let error = fx
        .start_with(choice("two", None), &shown.revision, false, git_fetcher())
        .unwrap_err();
    assert!(
        error.message.contains("changed since the preview"),
        "{}",
        error.message
    );
    // The PR moved.
    let mut moved = fx.prepare_request(choice("one", None), &shown.revision, false);
    moved.identity.head_sha = "e".repeat(40);
    let error = start_with(fx.locker(), moved, git_fetcher(), Arc::new(|| {})).unwrap_err();
    assert!(error.message.contains("new commits"), "{}", error.message);
    // The setup hook appeared.
    fx.set_hook(crate::extension::HookConfig::Script("true".into()));
    let error = fx
        .start_with(choice("one", None), &shown.revision, false, git_fetcher())
        .unwrap_err();
    assert!(
        error.message.contains("changed since the preview"),
        "{}",
        error.message
    );
    // The source feature is gone.
    fx.with(|gui| gui.app_for_workflow().store.projects[0].features.clear());
    assert!(fx.try_preview(choice("one", None)).is_err());
    assert!(!fx.worktree_dir("one").exists());
}

#[test]
fn the_resource_gate_asks_then_proceeds_once_approved_and_a_refusal_frees_the_slot() {
    let fx = fixture(Head::Local);
    fx.with(|gui| gui.app_for_workflow().config.low_memory_warn_mb = u64::MAX);
    let choice = choice("gated", None);
    let shown = fx.preview(choice.clone());
    assert!(shown.resource_warning.is_some());
    let error = fx
        .start_with(choice.clone(), &shown.revision, false, git_fetcher())
        .unwrap_err();
    assert_eq!(error.kind, GuiErrorKind::NeedsApproval);
    assert_eq!(fx.features().len(), 1);
    assert!(!fx.worktree_dir("gated").exists());
    let started = fx
        .start_with(choice, &shown.revision, true, git_fetcher())
        .unwrap();
    assert_eq!(started.status, PrepareStatus::Started);
    assert_eq!(
        wait_done(&started.operation_id).state,
        OperationState::Created
    );
}

// --------------------------------------------------------------- rollback

#[test]
fn a_failed_save_keeps_the_checkout_names_its_path_and_leaves_no_half_registered_feature() {
    let fx = fixture(Head::Local);
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let blocked = blocker.path().join("store.json");
    let gui = fx.gui.clone();
    fx.set_worktree(Wrapped {
        after_create: Box::new(move |_| {
            // Break persistence after the checkout exists and before it is registered.
            let mut gui = gui.lock().unwrap();
            let app = gui.app_for_workflow();
            app.db = None;
            app.store_path = blocked.clone();
        }),
    });
    let done = fx.create("unsaved");
    assert_eq!(done.state, OperationState::Failed);
    let path = fx.worktree_dir("unsaved");
    assert!(
        done.message.contains("Triage checkout retained at"),
        "{}",
        done.message
    );
    assert!(
        done.message.contains(path.to_str().unwrap()),
        "{}",
        done.message
    );
    assert!(
        done.message.contains("No feature was registered"),
        "{}",
        done.message
    );
    assert_eq!(done.retained_path.as_deref(), path.to_str());
    assert!(path.exists(), "never deleted automatically");
    assert!(done.created_target.is_none());
    assert_eq!(fx.features().len(), 1, "no half-registered feature");
}

#[test]
fn a_source_deleted_during_creation_keeps_the_checkout_and_registers_nothing() {
    let fx = fixture(Head::Local);
    let gui = fx.gui.clone();
    fx.set_worktree(Wrapped {
        after_create: Box::new(move |_| {
            gui.lock().unwrap().app_for_workflow().store.projects[0]
                .features
                .clear();
        }),
    });
    let done = fx.create("orphan");
    assert_eq!(done.state, OperationState::Failed, "{}", done.message);
    assert!(
        done.message.contains("Source feature removed"),
        "{}",
        done.message
    );
    assert!(fx.worktree_dir("orphan").exists());
    assert!(fx.features().is_empty());
}

#[test]
fn a_worktree_that_cannot_be_created_registers_nothing_and_says_so() {
    let fx = fixture(Head::Local);
    let mut ops = crate::traits::MockWorktreeOps::new();
    ops.expect_create_from()
        .returning(|_, _, _, _| anyhow::bail!("git worktree add failed: no space left"));
    fx.set_worktree(ops);
    let done = fx.create("no-space");
    assert_eq!(done.state, OperationState::Failed);
    assert!(done.message.contains("no space left"), "{}", done.message);
    assert!(
        done.message.contains("No feature was registered"),
        "{}",
        done.message
    );
    assert_eq!(fx.features().len(), 1);
}

#[test]
fn a_panicking_worker_reports_a_failure_and_frees_the_workflow() {
    let fx = fixture(Head::Local);
    fx.set_worktree(Wrapped {
        after_create: Box::new(|_| panic!("boom")),
    });
    let done = fx.create("panics");
    assert_eq!(done.state, OperationState::Failed);
    assert!(
        done.message.contains("stopped unexpectedly"),
        "{}",
        done.message
    );
    // The workflow's slot was released.
    fx.set_worktree(crate::worktree::WorktreeManager);
    let again = fx.create("recovered");
    assert_eq!(again.state, OperationState::Created, "{}", again.message);
}

#[test]
fn creation_never_pushes_or_writes_to_the_remote() {
    let fx = fixture(Head::Local);
    let remote = fx.dir.path().join("remote.git");
    let refs = |dir: &Path| run_git(dir, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let before = refs(&remote);
    assert_eq!(fx.create("quiet").state, OperationState::Created);
    assert_eq!(refs(&remote), before);
}

// ------------------------------------------------------- review follow-ups

fn pid_is_gone(pid: i32) -> bool {
    // A zombie still answers kill(0) but is already dead.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

fn wait_pid_file(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "no pid in {}", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_pid_gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid_is_gone(pid) {
        assert!(Instant::now() < deadline, "process {pid} survived");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_hook_that_backgrounds_a_daemon_neither_hangs_the_operation_nor_leaves_it_running() {
    let fx = fixture(Head::Local);
    let pid_file = fx.dir.path().join("daemon.pid");
    // The sleeper inherits the hook's stderr, which used to pin the reader.
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "sleep 300 & echo $! > '{}'",
        pid_file.display()
    )));
    let done = fx.create("daemonized");
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    wait_pid_gone(wait_pid_file(&pid_file));
    // The workflow's slot is free again.
    assert!(matches!(
        begin_operation(&fx.workflow, COMPANION_OPERATIONS),
        OperationStart::Started(_)
    ));
}

#[test]
fn cancelling_a_hook_with_a_backgrounded_daemon_kills_the_whole_group_promptly() {
    let fx = fixture(Head::Local);
    let pid_file = fx.dir.path().join("daemon.pid");
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "sleep 300 & echo $! > '{}'; sleep 300",
        pid_file.display()
    )));
    let id = fx.begin("daemon-cancel");
    let daemon = wait_pid_file(&pid_file);
    let cancelled_at = Instant::now();
    cancel(&id).unwrap();
    let done = wait_done(&id);
    assert_eq!(done.state, OperationState::Cancelled, "{}", done.message);
    assert!(cancelled_at.elapsed() < Duration::from_secs(10));
    wait_pid_gone(daemon);
}

#[test]
fn a_hook_without_a_confirmed_origin_branch_needs_explicit_approval() {
    // A fork PR: its branch is not on origin.
    let fx = fixture(Head::Nowhere);
    let marker = fx.dir.path().join("hook-ran");
    fx.set_hook(crate::extension::HookConfig::Script(format!(
        "touch '{}'",
        marker.display()
    )));
    let choice = choice("forked", None);
    let shown = fx.preview(choice.clone());
    assert!(
        shown
            .consequences
            .iter()
            .any(|line| line.contains("executes files from the PR checkout")),
        "{:?}",
        shown.consequences
    );
    let error = fx
        .start_with(choice, &shown.revision, false, git_fetcher())
        .unwrap_err();
    assert_eq!(error.kind, GuiErrorKind::NeedsApproval);
    assert!(error.message.contains("fork"), "{}", error.message);
    assert!(!marker.exists());
    assert!(!fx.worktree_dir("forked").exists());
    // The slot was released by the refusal.
    assert!(matches!(
        begin_operation(&fx.workflow, COMPANION_OPERATIONS),
        OperationStart::Started(_)
    ));
}

#[test]
fn an_approved_hook_on_an_unconfirmed_branch_runs_and_a_confirmed_one_needs_no_approval() {
    let fx = fixture(Head::NoRemote);
    fx.set_hook(crate::extension::HookConfig::Script(
        "touch hook-ran".into(),
    ));
    let choice = choice("approved", None);
    let shown = fx.preview(choice.clone());
    let started = fx
        .start_with(choice, &shown.revision, true, git_fetcher())
        .unwrap();
    let done = wait_done(&started.operation_id);
    assert_eq!(done.state, OperationState::Created, "{}", done.message);
    assert!(fx.worktree_dir("approved").join("hook-ran").exists());
    // Head::Local is on origin: the existing hook tests start with approved=false.
}

#[test]
fn the_revision_ignores_a_resource_warning_that_comes_and_goes() {
    let fx = fixture(Head::Local);
    let quiet = fx.preview(choice("steady", None));
    assert!(quiet.resource_warning.is_none());
    fx.with(|gui| gui.app_for_workflow().config.low_memory_warn_mb = u64::MAX);
    let warned = fx.preview(choice("steady", None));
    assert!(warned.resource_warning.is_some());
    assert_eq!(quiet.revision, warned.revision);
}

#[test]
fn a_head_reported_in_capitals_plans_the_same_base_as_lowercase() {
    let fx = fixture(Head::Local);
    let lower = fx.preview(choice("case", None));
    fx.with(|gui| {
        let ctx = gui.pr_workspace_context.as_mut().unwrap();
        let pr = ctx.pr.as_mut().unwrap();
        pr.head_sha = pr.head_sha.to_ascii_uppercase();
    });
    let identity = fx.with(|gui| catalog(gui, &fx.workflow).unwrap().identity);
    let request = PrDestinationRequest {
        workflow: fx.workflow.clone(),
        identity,
        choice: choice("case", None),
    };
    let upper = fx.with(|gui| preview_read(gui, &request)).unwrap().run();
    assert_eq!(
        upper.companion.unwrap().base_sha,
        lower.companion.unwrap().base_sha
    );
}

#[test]
fn branch_names_git_rewrites_are_refused() {
    let fx = fixture(Head::Local);
    for bad in ["@{-1}", "@{-2}", "@"] {
        let error = fx.try_preview(choice(bad, None)).unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::Conflict, "{bad:?}");
        assert!(error.message.contains("not a valid"), "{}", error.message);
    }
}

#[test]
fn two_workflows_racing_with_different_branch_names_cannot_both_register_the_same_head() {
    let fx = fixture(Head::Local);
    // Workflow B passed `plan` before A registered anything.
    let request = fx.request(choice("race-b", None));
    let planned_b = fx
        .with(|gui| {
            plan(
                gui,
                &request.workflow,
                &request.identity,
                &settings("race-b"),
                None,
                true,
            )
        })
        .unwrap();
    assert_eq!(fx.create("race-a").state, OperationState::Created);
    let state = Arc::new(OpState::new("race-b-op".into()));
    let job = Job {
        locker: fx.locker(),
        state: state.clone(),
        fetcher: git_fetcher(),
        worktree: Arc::new(crate::worktree::WorktreeManager),
        request: fx.prepare_request(choice("race-b", None), &planned_b.revision, false),
        settings: settings("race-b"),
        hook_choice: None,
        plan: planned_b,
        on_finish: Arc::new(|| {}),
    };
    job.execute();
    let view = state.view();
    assert_eq!(view.state, OperationState::Failed, "{}", view.message);
    assert!(
        view.message.contains("race-a") && view.message.contains("was created while"),
        "{}",
        view.message
    );
    assert!(view.created_target.is_none());
    assert!(
        view.retained_path.is_some(),
        "the checkout is kept and named"
    );
    assert!(fx.features().iter().all(|f| f.name != "race-b"));
}

#[test]
fn a_companion_creation_and_a_session_prepare_do_not_share_an_operation_slot() {
    let fx = fixture(Head::Local);
    let OperationStart::Started(session) = begin_operation(
        &fx.workflow,
        crate::gui_dedicated_session::SESSION_OPERATIONS,
    ) else {
        panic!("session claims its slot")
    };
    let choice = choice("separate", None);
    let shown = fx.preview(choice.clone());
    let started = fx
        .start_with(choice, &shown.revision, false, git_fetcher())
        .unwrap();
    assert_eq!(started.status, PrepareStatus::Started);
    assert_ne!(started.operation_id, session.operation_id);
    assert_eq!(
        wait_done(&started.operation_id).state,
        OperationState::Created
    );
    // And the reverse: while a companion holds its slot, a session can claim its own.
    let OperationStart::Started(companion) = begin_operation(&fx.workflow, COMPANION_OPERATIONS)
    else {
        panic!("companion slot is free again")
    };
    assert!(matches!(
        begin_operation(
            &fx.workflow,
            crate::gui_dedicated_session::SESSION_OPERATIONS
        ),
        OperationStart::Existing(_)
    ));
    drop(companion);
    drop(session);
}

#[test]
fn a_panicking_worker_still_tells_the_ui_it_finished() {
    let fx = fixture(Head::Local);
    fx.set_worktree(Wrapped {
        after_create: Box::new(|_| panic!("boom")),
    });
    let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = finished.clone();
    let choice = choice("tells-ui", None);
    let shown = fx.preview(choice.clone());
    let started = start_with(
        fx.locker(),
        fx.prepare_request(choice, &shown.revision, false),
        git_fetcher(),
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    assert_eq!(
        wait_done(&started.operation_id).state,
        OperationState::Failed
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while finished.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "on_finish never ran");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}
