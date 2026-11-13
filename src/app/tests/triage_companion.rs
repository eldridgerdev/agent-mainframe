//! Characterization tests for PR Triage's `New feature…` companion creation.
//!
//! These drive the TUI through its public entry points only
//! (`pr_review_open_triage_feature_setup`, `pr_review_triage_setup_confirm`),
//! against real git repositories and a worktree mock that performs a real
//! `git worktree add`. They pin what the TUI does today so the plan/register
//! split of `create_triage_feature` can be shown to change nothing.

use super::support::*;
use crate::app::*;
use crate::extension::{HookConfig, HookPrompt};
use crate::project::{AgentKind, SessionKind, TriageSource, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::{NamedTempFile, TempDir};

pub(super) fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

type CreateCalls = Arc<Mutex<Vec<(String, String, String)>>>;

pub(super) struct Fixture {
    pub app: App,
    pub repo: PathBuf,
    /// `(worktree name, new branch, base)` for every `create_from` call.
    pub calls: CreateCalls,
    pub c2: String,
    _dirs: Vec<TempDir>,
    _store_file: NamedTempFile,
}

pub(super) fn expect_feature_start(tmux: &mut MockTmuxOps) {
    tmux.expect_session_exists().returning(|_| false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
}

/// A repo on `main` (commits c1, c2) with a `pr-head` branch at c2, the source
/// feature checked out at the repo root, and PR #7 open on it.
///
/// `start` controls whether tmux accepts a feature start (success paths) or
/// has no expectations at all (failure paths: any start would panic).
pub(super) fn fixture(start: bool, repo_root_ok: bool) -> Fixture {
    let root = TempDir::new().unwrap();
    let wt_root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "c1"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "c2"]);
    let c2 = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "pr-head"]);

    let calls: CreateCalls = Arc::default();
    let mut worktree = MockWorktreeOps::new();
    let repo_for_root = repo.clone();
    worktree.expect_repo_root().returning(move |_| {
        if repo_root_ok {
            Ok(repo_for_root.clone())
        } else {
            Err(anyhow::anyhow!("not a repository"))
        }
    });
    let calls_for_create = calls.clone();
    let repo_for_create = repo.clone();
    let wt_base = wt_root.path().to_path_buf();
    worktree
        .expect_create_from()
        .returning(move |_, name, branch, base| {
            calls_for_create.lock().unwrap().push((
                name.to_string(),
                branch.to_string(),
                base.to_string(),
            ));
            let path = wt_base.join(name);
            let out = std::process::Command::new("git")
                .args(["worktree", "add", "-q", "-b", branch])
                .arg(&path)
                .arg(base)
                .current_dir(&repo_for_create)
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            Ok(path)
        });

    let mut tmux = MockTmuxOps::new();
    if start {
        expect_feature_start(&mut tmux);
    }

    let store = store_with_repo(repo.clone(), ProjectStatus::Stopped);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.clone();
        state.review.pr.head_ref = "pr-head".to_string();
        state.review.pr.head_sha = c2.clone();
        state.checked_out_branch = Some("main".to_string());
    }
    Fixture {
        app,
        repo,
        calls,
        c2,
        _dirs: vec![root, wt_root],
        _store_file: store_file,
    }
}

pub(super) fn set_pr(app: &mut App, head_ref: &str, head_sha: &str) {
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.pr.head_ref = head_ref.to_string();
        state.review.pr.head_sha = head_sha.to_string();
    }
}

pub(super) fn open_setup(app: &mut App) {
    app.pr_review_open_triage_feature_setup(false);
}

pub(super) fn setup_mut(app: &mut App) -> &mut crate::app::TriageFeatureSetupState {
    match &mut app.mode {
        AppMode::PrReview(state) => state
            .new_feature_setup
            .as_mut()
            .expect("setup overlay should be open"),
        _ => panic!("expected PrReview"),
    }
}

fn setup_error(app: &App) -> Option<String> {
    match &app.mode {
        AppMode::PrReview(state) => state
            .new_feature_setup
            .as_ref()
            .and_then(|setup| setup.error.clone()),
        _ => panic!("expected PrReview"),
    }
}

fn setup_open(app: &App) -> bool {
    matches!(&app.mode, AppMode::PrReview(s) if s.new_feature_setup.is_some())
}

fn companions<'a>(app: &'a App, name: &str) -> Vec<&'a crate::project::Feature> {
    app.store.projects[0]
        .features
        .iter()
        .filter(|f| f.name == name)
        .collect()
}

fn feature_count(app: &App) -> usize {
    app.store.projects[0].features.len()
}

fn toast_texts(app: &App) -> Vec<String> {
    app.toasts.iter().map(|t| t.message.clone()).collect()
}

fn branches(repo: &Path) -> String {
    git(repo, &["branch", "--format=%(refname:short)"])
}

// ------------------------------------------------------------- success path

#[test]
fn creates_a_started_companion_seeded_from_the_local_pr_branch() {
    let mut fx = fixture(true, true);
    open_setup(&mut fx.app);
    assert_eq!(setup_mut(&mut fx.app).branch, "pr-head-triage");
    {
        let setup = setup_mut(&mut fx.app);
        setup.mode = VibeMode::Vibe;
        setup.review = true;
        setup.enable_chrome = true;
    }
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", toast_texts(&fx.app));
    // The local PR branch contains the head, so it is the base (unpushed work
    // is not dropped), by name rather than by sha.
    let calls = fx.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "my-project-pr-head-triage");
    assert_eq!(calls[0].1, "pr-head-triage");
    assert_eq!(calls[0].2, "pr-head");

    let found = companions(&fx.app, "pr-head-triage");
    assert_eq!(found.len(), 1);
    let feature = found[0];
    assert_eq!(feature.branch, "pr-head-triage");
    assert!(feature.is_worktree);
    assert_eq!(feature.mode, VibeMode::Vibe);
    assert!(feature.review);
    assert!(feature.enable_chrome);
    assert!(!feature.plan_mode, "triage never defers into planning");
    assert_eq!(feature.agent, AgentKind::default());
    // Compared canonically: macOS temp dirs sit behind a /var -> /private/var link.
    let listed = git(&fx.repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .find(|p| p.ends_with("my-project-pr-head-triage"))
        .map(PathBuf::from)
        .expect("git lists the new worktree");
    assert_eq!(
        feature.workdir.canonicalize().unwrap(),
        listed.canonicalize().unwrap()
    );
    assert_eq!(
        feature.triage_source,
        Some(TriageSource {
            pr_number: 7,
            source_feature_id: "feat-1".to_string(),
            pr_branch: "pr-head".to_string(),
            base_sha: fx.c2.clone(),
        })
    );
    // One session, carrying the triage label.
    assert_eq!(feature.sessions.len(), 1);
    assert_eq!(
        feature.sessions[0].label,
        crate::app::pr_review::TRIAGE_SESSION_LABEL
    );
    assert!(matches!(feature.sessions[0].kind, SessionKind::Claude));
    // It was started (the agent launch is the last step).
    assert_ne!(feature.status, ProjectStatus::Stopped);
    // The project is expanded so the new row is visible.
    assert!(!fx.app.store.projects[0].collapsed);
    // The pane moved on with the new target.
    match &fx.app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::NewFeature
            );
            assert!(state.fix_target_picked);
            assert_eq!(state.review_harness, Some(AgentKind::default()));
        }
        _ => panic!("expected PrReview"),
    }
    assert!(
        toast_texts(&fx.app)
            .iter()
            .any(|t| t.contains("Triage feature 'pr-head-triage' created")),
        "{:?}",
        toast_texts(&fx.app)
    );
    // The registered feature was persisted.
    let saved = crate::project::ProjectStore::load(&fx.app.store_path).unwrap();
    assert!(
        saved.projects[0]
            .features
            .iter()
            .any(|f| f.name == "pr-head-triage" && f.triage_source.is_some())
    );
}

#[test]
fn branch_name_is_trimmed_before_it_names_the_feature_and_worktree() {
    let mut fx = fixture(true, true);
    open_setup(&mut fx.app);
    setup_mut(&mut fx.app).branch = "  custom-fixes \n".to_string();
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    let calls = fx.calls.lock().unwrap().clone();
    assert_eq!(calls[0].0, "my-project-custom-fixes");
    assert_eq!(calls[0].1, "custom-fixes");
    assert_eq!(companions(&fx.app, "custom-fixes").len(), 1);
}

// ---------------------------------------------------------------- base pick

#[test]
fn falls_back_to_the_pr_head_sha_when_the_pr_branch_is_missing_locally() {
    let mut fx = fixture(true, true);
    // The PR head lives only as an object: its branch never existed here.
    git(&fx.repo, &["checkout", "-q", "-b", "scratch", "main"]);
    std::fs::write(fx.repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(&fx.repo, &["add", "-A"]);
    git(&fx.repo, &["commit", "-qm", "pr only"]);
    let sha = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(&fx.repo, &["branch", "-D", "scratch"]);
    set_pr(&mut fx.app, "not-here", &sha);

    open_setup(&mut fx.app);
    assert_eq!(setup_mut(&mut fx.app).branch, "not-here-triage");
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    assert_eq!(fx.calls.lock().unwrap()[0].2, sha);
    let link = companions(&fx.app, "not-here-triage")[0]
        .triage_source
        .clone()
        .unwrap();
    assert_eq!(link.pr_branch, "not-here");
    assert_eq!(link.base_sha, sha);
}

#[test]
fn falls_back_to_the_pr_head_sha_when_the_local_branch_is_behind() {
    let mut fx = fixture(true, true);
    git(&fx.repo, &["checkout", "-q", "-b", "scratch", "main"]);
    std::fs::write(fx.repo.join("b.txt"), "x\n").unwrap();
    git(&fx.repo, &["add", "-A"]);
    git(&fx.repo, &["commit", "-qm", "ahead"]);
    let sha = git(&fx.repo, &["rev-parse", "HEAD"]);
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(&fx.repo, &["branch", "-D", "scratch"]);
    // `pr-head` exists but does not contain `sha`.
    set_pr(&mut fx.app, "pr-head", &sha);

    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    assert_eq!(fx.calls.lock().unwrap()[0].2, sha);
    assert_eq!(
        companions(&fx.app, "pr-head-triage")[0]
            .triage_source
            .as_ref()
            .unwrap()
            .base_sha,
        sha
    );
}

#[test]
fn an_empty_head_ref_falls_back_to_the_source_features_branch() {
    let mut fx = fixture(true, true);
    // A cached PR row from before `head_ref` existed: the checked-out branch is
    // the only thing known. Make the source feature's branch a real one.
    fx.app.store.projects[0].features[0].branch = "main".to_string();
    set_pr(&mut fx.app, "", &fx.c2.clone());

    open_setup(&mut fx.app);
    assert_eq!(setup_mut(&mut fx.app).branch, "-triage");
    setup_mut(&mut fx.app).branch = "legacy-triage".to_string();
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    assert_eq!(fx.calls.lock().unwrap()[0].2, "main");
    let link = companions(&fx.app, "legacy-triage")[0]
        .triage_source
        .clone()
        .unwrap();
    assert_eq!(link.pr_branch, "main");
    assert_eq!(link.base_sha, fx.c2);
}

#[test]
fn a_padded_head_ref_is_trimmed_in_the_recorded_link() {
    let mut fx = fixture(true, true);
    let sha = fx.c2.clone();
    set_pr(&mut fx.app, " pr-head ", &sha);
    open_setup(&mut fx.app);
    setup_mut(&mut fx.app).branch = "padded-triage".to_string();
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    assert_eq!(
        companions(&fx.app, "padded-triage")[0]
            .triage_source
            .as_ref()
            .unwrap()
            .pr_branch,
        "pr-head"
    );
}

// ------------------------------------------------------------------ naming

#[test]
fn suggested_branch_skips_taken_feature_names_and_git_branches() {
    let mut fx = fixture(false, true);
    let mut taken = fx.app.store.projects[0].features[0].clone();
    taken.id = "feat-taken".to_string();
    taken.name = "pr-head-triage".to_string();
    taken.workdir = PathBuf::from("/tmp/elsewhere-1");
    fx.app.store.projects[0].features.push(taken);

    open_setup(&mut fx.app);
    assert_eq!(setup_mut(&mut fx.app).branch, "pr-head-triage-2");
    fx.app.pr_review_triage_setup_cancel();

    // A git branch (no feature) also counts as taken: -2 is a branch, so -3.
    git(&fx.repo, &["branch", "pr-head-triage-2"]);
    open_setup(&mut fx.app);
    assert_eq!(setup_mut(&mut fx.app).branch, "pr-head-triage-3");
    fx.app.pr_review_triage_setup_cancel();

    // A git branch alone also displaces the bare suggestion.
    fx.app.store.projects[0].features.pop();
    git(&fx.repo, &["branch", "pr-head-triage"]);
    open_setup(&mut fx.app);
    assert_eq!(setup_mut(&mut fx.app).branch, "pr-head-triage-3");
}

#[test]
fn feature_name_collision_is_normalized() {
    let mut fx = fixture(false, true);
    let mut taken = fx.app.store.projects[0].features[0].clone();
    taken.id = "feat-taken".to_string();
    taken.name = "pr-head-triage".to_string();
    taken.workdir = PathBuf::from("/tmp/elsewhere-2");
    fx.app.store.projects[0].features.push(taken);

    open_setup(&mut fx.app);
    setup_mut(&mut fx.app).branch = "PR-HEAD-TRIAGE".to_string();
    let before = feature_count(&fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).expect("collision stays inline");
    assert!(err.contains("already exists in 'my-project'"), "{err}");
    assert!(setup_open(&fx.app));
    assert!(fx.calls.lock().unwrap().is_empty());
    assert_eq!(feature_count(&fx.app), before);
}

// --------------------------------------------------------------- failures

#[test]
fn empty_branch_is_refused_inline_without_touching_git() {
    let mut fx = fixture(false, true);
    open_setup(&mut fx.app);
    setup_mut(&mut fx.app).branch = "   ".to_string();
    let before = (feature_count(&fx.app), branches(&fx.repo));
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert_eq!(
        setup_error(&fx.app).as_deref(),
        Some("Branch name cannot be empty")
    );
    assert!(fx.calls.lock().unwrap().is_empty());
    assert_eq!((feature_count(&fx.app), branches(&fx.repo)), before);
}

#[test]
fn an_unknown_workdir_is_refused_inline() {
    let mut fx = fixture(false, true);
    open_setup(&mut fx.app);
    if let AppMode::PrReview(state) = &mut fx.app.mode {
        state.workdir = PathBuf::from("/tmp/not-a-feature-workdir");
    }
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert_eq!(
        setup_error(&fx.app).as_deref(),
        Some("could not find the feature for this PR")
    );
    assert!(fx.calls.lock().unwrap().is_empty());
}

#[test]
fn a_non_git_project_is_refused_inline() {
    let mut fx = fixture(false, false);
    open_setup(&mut fx.app);
    // `open_setup` needs the agent list, not the repo root.
    assert!(setup_open(&fx.app));
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).unwrap();
    assert!(err.contains("needs a git repository"), "{err}");
    assert!(fx.calls.lock().unwrap().is_empty());
}

#[test]
fn a_harness_the_workspace_does_not_allow_is_refused_inline() {
    let mut fx = fixture(false, true);
    fx.app.store.available_harnesses = vec![AgentKind::Claude];
    open_setup(&mut fx.app);
    {
        let setup = setup_mut(&mut fx.app);
        setup.agents = vec![AgentKind::Codex];
        setup.agent_index = 0;
        setup.mode = VibeMode::Vibe;
    }
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).unwrap();
    assert!(
        err.contains("Harness 'Codex' is not allowed for this workspace"),
        "{err}"
    );
    assert!(fx.calls.lock().unwrap().is_empty());
}

#[test]
fn a_mode_the_harness_cannot_run_is_refused_inline() {
    let mut fx = fixture(false, true);
    open_setup(&mut fx.app);
    {
        let setup = setup_mut(&mut fx.app);
        setup.agents = vec![AgentKind::Codex];
        setup.agent_index = 0;
        setup.mode = VibeMode::Vibeless;
    }
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).unwrap();
    assert!(err.contains("Codex does not support Vibeless"), "{err}");
    assert!(fx.calls.lock().unwrap().is_empty());
}

#[test]
fn an_unresolvable_pr_head_is_refused_before_any_worktree_exists() {
    let mut fx = fixture(false, true);
    // No such branch and an abbreviated sha that resolves nowhere (the fetch
    // of an unreachable `origin` fails quietly).
    set_pr(&mut fx.app, "ghost", "deadbee");
    open_setup(&mut fx.app);
    let before = branches(&fx.repo);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert_eq!(
        setup_error(&fx.app).as_deref(),
        Some("could not resolve a base commit for the PR head")
    );
    assert!(fx.calls.lock().unwrap().is_empty());
    assert_eq!(branches(&fx.repo), before);
}

#[test]
fn a_full_length_unknown_sha_is_only_rejected_by_git_at_worktree_creation() {
    // `git rev-parse --verify` accepts a well-formed 40-hex id without
    // checking the object exists, so the base "resolves" and the failure
    // surfaces from `git worktree add`. Pinned as-is: the split must not
    // change when this is caught for the TUI.
    let mut fx = fixture(false, true);
    set_pr(
        &mut fx.app,
        "ghost",
        "0123456789abcdef0123456789abcdef01234567",
    );
    open_setup(&mut fx.app);
    let before = feature_count(&fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).unwrap();
    assert!(err.contains("git worktree add failed"), "{err}");
    assert_eq!(fx.calls.lock().unwrap().len(), 1);
    assert_eq!(feature_count(&fx.app), before);
}

#[test]
fn a_failed_worktree_creation_leaves_the_store_and_agents_alone() {
    let mut fx = fixture(false, true);
    // The branch name is free as a feature but taken as a git branch, so
    // `git worktree add -b` refuses.
    git(&fx.repo, &["branch", "taken-in-git"]);
    open_setup(&mut fx.app);
    setup_mut(&mut fx.app).branch = "taken-in-git".to_string();
    let before = feature_count(&fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).unwrap();
    assert!(err.contains("git worktree add failed"), "{err}");
    assert!(setup_open(&fx.app), "the overlay stays open for correction");
    assert_eq!(fx.calls.lock().unwrap().len(), 1);
    assert_eq!(feature_count(&fx.app), before);
    // The tmux mock has no expectations, so reaching a start would have panicked.
}

#[test]
fn a_failed_save_is_reported_inline_and_nothing_is_started() {
    let mut fx = fixture(false, true);
    let blocker = NamedTempFile::new().unwrap();
    // A path *under a regular file* can never be written.
    fx.app.store_path = blocker.path().join("store.json");
    open_setup(&mut fx.app);
    let before = feature_count(&fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    let err = setup_error(&fx.app).unwrap();
    assert!(
        err.contains("delete the retained checkout and its branch"),
        "{err}"
    );
    assert!(setup_open(&fx.app));
    assert_eq!(fx.calls.lock().unwrap().len(), 1);
    // The failed row is not left behind in the store.
    assert_eq!(feature_count(&fx.app), before);
    assert!(companions(&fx.app, "pr-head-triage").is_empty());
    // The tmux mock has no expectations: no start was attempted.
}

// ------------------------------------------------------------------- hooks

fn hook_fixture(hook: HookConfig) -> Fixture {
    let mut fx = fixture(true, true);
    fx.app.config.extension.lifecycle_hooks.on_worktree_created = Some(hook);
    fx
}

fn created_workdir(app: &App, name: &str) -> PathBuf {
    companions(app, name)[0].workdir.clone()
}

#[test]
fn the_worktree_hook_runs_in_the_new_checkout_before_the_feature_starts() {
    let mut fx = hook_fixture(HookConfig::Script("echo ran > hook-ran.txt".into()));
    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    let workdir = created_workdir(&fx.app, "pr-head-triage");
    assert_eq!(
        std::fs::read_to_string(workdir.join("hook-ran.txt"))
            .unwrap()
            .trim(),
        "ran"
    );
    assert!(
        !toast_texts(&fx.app)
            .iter()
            .any(|t| t.contains("hook failed"))
    );
}

#[test]
fn a_failing_worktree_hook_warns_but_the_companion_is_still_created_and_started() {
    let mut fx = hook_fixture(HookConfig::Script("echo boom >&2; exit 3".into()));
    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    let found = companions(&fx.app, "pr-head-triage");
    assert_eq!(found.len(), 1);
    assert_ne!(found[0].status, ProjectStatus::Stopped);
    assert!(
        toast_texts(&fx.app)
            .iter()
            .any(|t| t == "Triage worktree created, but its on_worktree_created hook failed"),
        "{:?}",
        toast_texts(&fx.app)
    );
}

#[test]
fn a_prompting_hook_runs_without_a_choice_in_the_tui() {
    let mut fx = hook_fixture(HookConfig::WithPrompt {
        script: "printf '%s' \"${AMF_HOOK_CHOICE-unset}\" > choice.txt".into(),
        prompt: HookPrompt {
            title: "Pick".into(),
            options: vec!["a".into(), "b".into()],
        },
    });
    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    let workdir = created_workdir(&fx.app, "pr-head-triage");
    assert_eq!(
        std::fs::read_to_string(workdir.join("choice.txt")).unwrap(),
        "unset"
    );
}

#[test]
fn a_failing_prompting_hook_warns_but_the_companion_is_still_created_and_started() {
    let mut fx = hook_fixture(HookConfig::WithPrompt {
        script: "echo boom >&2; exit 3".into(),
        prompt: HookPrompt {
            title: "Pick".into(),
            options: vec!["a".into(), "b".into()],
        },
    });
    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(!setup_open(&fx.app), "{:?}", setup_error(&fx.app));
    let found = companions(&fx.app, "pr-head-triage");
    assert_eq!(found.len(), 1);
    assert_ne!(found[0].status, ProjectStatus::Stopped);
    assert!(
        toast_texts(&fx.app)
            .iter()
            .any(|t| t == "Triage worktree created, but its on_worktree_created hook failed"),
        "{:?}",
        toast_texts(&fx.app)
    );
}

#[test]
fn no_hook_configured_runs_nothing() {
    let mut fx = fixture(true, true);
    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();
    assert!(!setup_open(&fx.app));
    assert!(
        !toast_texts(&fx.app)
            .iter()
            .any(|t| t.contains("hook failed"))
    );
}

// ------------------------------------------------------------------ adopt

#[test]
fn a_created_companion_is_adopted_again_on_the_next_visit() {
    let mut fx = fixture(true, true);
    open_setup(&mut fx.app);
    fx.app.pr_review_triage_setup_confirm().unwrap();
    assert!(!setup_open(&fx.app));

    // Re-enter the pane for the same PR: the created link is what adoption
    // matches on.
    enter_pr_review_for_feature(&mut fx.app, 1);
    if let AppMode::PrReview(state) = &mut fx.app.mode {
        state.workdir = fx.repo.clone();
    }
    fx.app.adopt_existing_triage_feature();
    match &fx.app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::NewFeature
            );
            assert!(state.fix_target_picked);
            assert_eq!(state.review_harness, Some(AgentKind::default()));
        }
        _ => panic!("expected PrReview"),
    }
}

// ======================================================================
// Plan / register split: new behavior of the extracted steps.
// ======================================================================

use crate::app::triage_feature::{
    FetchPolicy, TriageCompanionPlan, TriageCompanionSpec, TriagePrFacts,
};

fn facts(fx: &Fixture) -> TriagePrFacts {
    TriagePrFacts {
        pr_number: 7,
        head_sha: fx.c2.clone(),
        head_ref: "pr-head".to_string(),
    }
}

fn spec(branch: &str) -> TriageCompanionSpec {
    TriageCompanionSpec {
        branch: branch.to_string(),
        agent: AgentKind::Claude,
        mode: VibeMode::Vibe,
        review: false,
        design_review: false,
        enable_chrome: false,
    }
}

fn plan_with(
    fx: &Fixture,
    branch: &str,
    hook: Option<&HookConfig>,
    choice: Option<&str>,
    require: bool,
    fetch: FetchPolicy,
) -> anyhow::Result<TriageCompanionPlan> {
    fx.app.plan_triage_companion(
        "feat-1",
        &facts(fx),
        &spec(branch),
        hook,
        choice,
        require,
        fetch,
    )
}

fn plan(fx: &Fixture, branch: &str) -> TriageCompanionPlan {
    plan_with(fx, branch, None, None, false, FetchPolicy::Never).unwrap()
}

/// Every path under `.git`, sorted: catches a fetch, ref, worktree or lock
/// that a "read-only" step left behind.
fn git_dir_paths(repo: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            out.push(path.display().to_string());
            if path.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo.join(".git"), &mut out);
    out.sort();
    out
}

struct Snapshot {
    store: String,
    git_paths: Vec<String>,
    refs: String,
    worktrees: String,
    status: String,
    toasts: Vec<String>,
}

fn snapshot(fx: &Fixture) -> Snapshot {
    Snapshot {
        store: serde_json::to_string(&fx.app.store).unwrap(),
        git_paths: git_dir_paths(&fx.repo),
        refs: git(&fx.repo, &["for-each-ref"]),
        worktrees: git(&fx.repo, &["worktree", "list", "--porcelain"]),
        status: git(&fx.repo, &["status", "--porcelain"]),
        toasts: toast_texts(&fx.app),
    }
}

fn assert_unchanged(before: &Snapshot, after: &Snapshot) {
    assert_eq!(before.store, after.store, "store mutated");
    assert_eq!(
        before.git_paths, after.git_paths,
        ".git gained or lost files"
    );
    assert_eq!(before.refs, after.refs, "refs mutated");
    assert_eq!(before.worktrees, after.worktrees, "worktrees mutated");
    assert_eq!(before.status, after.status, "working tree mutated");
    assert_eq!(before.toasts, after.toasts, "toasts emitted");
}

#[test]
fn planning_changes_nothing_on_disk_in_git_or_in_the_store() {
    let fx = fixture(false, true);
    let hook = HookConfig::Script("echo SHOULD-NOT-RUN > planned-hook.txt".into());
    let before = snapshot(&fx);

    for fetch in [FetchPolicy::Never, FetchPolicy::IfMissing] {
        let p = plan_with(&fx, "plan-only", Some(&hook), None, false, fetch).unwrap();
        assert_eq!(p.spec.branch, "plan-only");
    }
    // Failing plans are read-only too.
    assert!(plan_with(&fx, "  ", None, None, false, FetchPolicy::Never).is_err());
    assert!(plan_with(&fx, "main", None, None, false, FetchPolicy::Never).is_ok());

    assert_unchanged(&before, &snapshot(&fx));
    assert!(fx.calls.lock().unwrap().is_empty(), "no worktree created");
    assert!(!fx.repo.join("planned-hook.txt").exists(), "hook ran");
}

#[test]
fn planning_without_fetch_never_writes_to_git_even_for_a_missing_head() {
    let fx = fixture(false, true);
    let before = snapshot(&fx);
    let mut f = facts(&fx);
    f.head_ref = "ghost".to_string();
    f.head_sha = "deadbee".to_string();

    let err = fx
        .app
        .plan_triage_companion(
            "feat-1",
            &f,
            &spec("x"),
            None,
            None,
            false,
            FetchPolicy::Never,
        )
        .unwrap_err();

    assert!(
        err.to_string().contains("could not resolve a base commit"),
        "{err}"
    );
    assert_unchanged(&before, &snapshot(&fx));
}

#[test]
fn a_plan_reports_whether_its_base_commit_exists_locally() {
    let fx = fixture(false, true);
    assert!(plan(&fx, "present").base_commit_present);

    // `rev-parse --verify` accepts any well-formed full id, so the TUI's base
    // "resolves" even when the object is absent; the plan says so explicitly
    // and leaves failing closed to its caller.
    let mut f = facts(&fx);
    f.head_ref = "ghost".to_string();
    f.head_sha = "0123456789abcdef0123456789abcdef01234567".to_string();
    let before = snapshot(&fx);
    let p = fx
        .app
        .plan_triage_companion(
            "feat-1",
            &f,
            &spec("x"),
            None,
            None,
            false,
            FetchPolicy::Never,
        )
        .unwrap();
    assert_eq!(p.base, f.head_sha);
    assert!(!p.base_commit_present);
    assert_unchanged(&before, &snapshot(&fx));
}

#[test]
fn a_plan_reports_names_base_and_link_inputs() {
    let fx = fixture(false, true);
    let p = plan(&fx, "  pr-head-triage ");

    assert_eq!(p.spec.branch, "pr-head-triage");
    assert_eq!(p.worktree_name, "my-project-pr-head-triage");
    assert_eq!(p.base, "pr-head");
    assert_eq!(p.base_sha, fx.c2);
    assert_eq!(p.pr_branch, "pr-head");
    assert_eq!(p.pr_number, 7);
    assert_eq!(p.source_feature_id, "feat-1");
    assert_eq!(p.source_workdir, fx.repo);
    assert_eq!(p.project_name, "my-project");
    assert!(!p.local_branch_exists);
    assert!(p.existing_companion.is_none());
    assert!(p.hook.is_none() && !p.hook_choice_required);

    git(&fx.repo, &["branch", "already-a-branch"]);
    assert!(plan(&fx, "already-a-branch").local_branch_exists);
}

#[test]
fn a_plan_names_an_existing_companion_for_the_same_pr_and_source() {
    let mut fx = fixture(false, true);
    let first = plan(&fx, "first-triage");
    fx.app.register_triage_companion(&first).unwrap();

    let second = plan(&fx, "second-triage");
    let existing = second.existing_companion.expect("companion is reported");
    assert_eq!(existing.name, "first-triage");

    // A different PR number is not the same companion.
    let mut other = facts(&fx);
    other.pr_number = 99;
    let p = fx
        .app
        .plan_triage_companion(
            "feat-1",
            &other,
            &spec("third"),
            None,
            None,
            false,
            FetchPolicy::Never,
        )
        .unwrap();
    assert!(p.existing_companion.is_none());
}

#[test]
fn plan_refuses_with_the_same_reasons_the_tui_shows() {
    let mut fx = fixture(false, true);
    let err = |r: anyhow::Result<TriageCompanionPlan>| r.unwrap_err().to_string();

    assert_eq!(
        err(plan_with(&fx, "", None, None, false, FetchPolicy::Never)),
        "Branch name cannot be empty"
    );
    assert!(
        err(fx.app.plan_triage_companion(
            "no-such-feature",
            &facts(&fx),
            &spec("x"),
            None,
            None,
            false,
            FetchPolicy::Never
        ))
        .contains("could not find the feature")
    );
    // Same normalized name as the source feature.
    assert!(
        err(plan_with(
            &fx,
            "MY-FEAT",
            None,
            None,
            false,
            FetchPolicy::Never
        ))
        .contains("already exists in 'my-project'")
    );
    fx.app.store.available_harnesses = vec![AgentKind::Codex];
    assert!(
        err(plan_with(&fx, "x", None, None, false, FetchPolicy::Never))
            .contains("not allowed for this workspace")
    );
}

#[test]
fn hook_choice_is_checked_at_plan_time_and_carried_into_the_plan() {
    let fx = fixture(false, true);
    let hook = HookConfig::WithPrompt {
        script: "true".into(),
        prompt: HookPrompt {
            title: "Pick".into(),
            options: vec!["a".into(), "b".into()],
        },
    };
    let missing = plan_with(&fx, "x", Some(&hook), None, true, FetchPolicy::Never)
        .unwrap_err()
        .to_string();
    assert!(missing.contains("requires a choice"), "{missing}");
    let wrong = plan_with(&fx, "x", Some(&hook), Some("zzz"), true, FetchPolicy::Never)
        .unwrap_err()
        .to_string();
    assert!(wrong.contains("Invalid hook_choice"), "{wrong}");

    let ok = plan_with(&fx, "x", Some(&hook), Some("b"), true, FetchPolicy::Never).unwrap();
    assert!(ok.hook_choice_required);
    assert_eq!(ok.hook_choice.as_deref(), Some("b"));
    // The TUI policy: not required, so no choice is fine.
    assert!(plan_with(&fx, "x", Some(&hook), None, false, FetchPolicy::Never).is_ok());
}

// ----------------------------------------------------------------- register

#[test]
fn register_creates_a_stopped_linked_companion_and_never_starts_it() {
    // No tmux expectations: starting the agent would panic.
    let mut fx = fixture(false, true);
    let p = plan(&fx, "reg-triage");
    let created = fx.app.register_triage_companion(&p).unwrap();

    assert_eq!(created.name, "reg-triage");
    assert!(created.hook_warning.is_none());
    let (pi, fi) = fx
        .app
        .store
        .locate_feature_by_id(Some(&created.project_id), &created.feature_id)
        .unwrap();
    let feature = &fx.app.store.projects[pi].features[fi];
    assert_eq!(feature.status, ProjectStatus::Stopped);
    assert_eq!(
        feature.triage_source,
        Some(TriageSource {
            pr_number: 7,
            source_feature_id: "feat-1".into(),
            pr_branch: "pr-head".into(),
            base_sha: fx.c2.clone(),
        })
    );
    assert_eq!(feature.sessions.len(), 1);
    assert_eq!(feature.sessions[0].id, created.session_id);
    assert_eq!(
        feature.sessions[0].label,
        crate::app::pr_review::TRIAGE_SESSION_LABEL
    );
    assert!(feature.workdir.is_dir());
    let saved = crate::project::ProjectStore::load(&fx.app.store_path).unwrap();
    assert!(
        saved.projects[0]
            .features
            .iter()
            .any(|f| f.id == created.feature_id)
    );
}

#[test]
fn register_runs_the_planned_hook_with_the_previewed_choice_not_a_later_config() {
    let mut fx = fixture(false, true);
    let planned = HookConfig::WithPrompt {
        script: "printf '%s' \"$AMF_HOOK_CHOICE\" > planned.txt".into(),
        prompt: HookPrompt {
            title: "Pick".into(),
            options: vec!["a".into(), "b".into()],
        },
    };
    let p = plan_with(
        &fx,
        "hooked",
        Some(&planned),
        Some("b"),
        true,
        FetchPolicy::Never,
    )
    .unwrap();
    // The config changes after the preview; the plan must win.
    fx.app.config.extension.lifecycle_hooks.on_worktree_created =
        Some(HookConfig::Script("echo later > later.txt".into()));

    let created = fx.app.register_triage_companion(&p).unwrap();

    let workdir = fx
        .app
        .store
        .locate_feature_by_id(None, &created.feature_id)
        .map(|(pi, fi)| fx.app.store.projects[pi].features[fi].workdir.clone())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workdir.join("planned.txt")).unwrap(),
        "b"
    );
    assert!(!workdir.join("later.txt").exists());
}

#[test]
fn a_failed_hook_is_reported_and_the_companion_is_kept_stopped() {
    let mut fx = fixture(false, true);
    let hook = HookConfig::Script("echo broke >&2; exit 4".into());
    let p = plan_with(&fx, "kept", Some(&hook), None, false, FetchPolicy::Never).unwrap();
    let created = fx.app.register_triage_companion(&p).unwrap();

    assert!(created.hook_warning.as_deref().unwrap().contains("broke"));
    assert_eq!(
        companions(&fx.app, "kept")[0].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn registration_rolls_back_when_the_save_fails_and_names_the_retained_checkout() {
    let mut fx = fixture(false, true);
    let p = plan(&fx, "rollback-save");
    let before_ids: Vec<String> = fx.app.store.projects[0]
        .features
        .iter()
        .map(|f| f.id.clone())
        .collect();
    let blocker = NamedTempFile::new().unwrap();
    fx.app.store_path = blocker.path().join("store.json");

    let err = fx
        .app
        .register_triage_companion(&p)
        .unwrap_err()
        .to_string();

    assert!(err.contains("Triage checkout retained at"), "{err}");
    let after_ids: Vec<String> = fx.app.store.projects[0]
        .features
        .iter()
        .map(|f| f.id.clone())
        .collect();
    assert_eq!(before_ids, after_ids, "no half-registered feature");
    // The checkout exists and is named, never silently deleted.
    let checkout = err
        .rsplit("retained at ")
        .next()
        .unwrap()
        .trim()
        .to_string();
    assert!(Path::new(&checkout).is_dir(), "{checkout}");
}

#[test]
fn the_final_guard_refuses_a_vanished_source_and_names_the_checkout() {
    let mut fx = fixture(false, true);
    let p = plan(&fx, "orphaned");
    fx.app.store.projects[0].features.clear();
    let dir = tempfile::tempdir().unwrap();

    let err = fx
        .app
        .register_created_triage_companion(&p, dir.path().to_path_buf())
        .unwrap_err()
        .to_string();

    assert!(err.contains("Source feature removed"), "{err}");
    assert!(err.contains("Triage checkout retained at"), "{err}");
    assert!(fx.app.store.projects[0].features.is_empty());
}

#[test]
fn the_final_guard_refuses_a_taken_name() {
    let mut fx = fixture(false, true);
    let p = plan(&fx, "raced");
    let mut taken = fx.app.store.projects[0].features[0].clone();
    taken.id = "feat-raced".into();
    taken.name = "raced".into();
    taken.workdir = PathBuf::from("/tmp/elsewhere-raced");
    fx.app.store.projects[0].features.push(taken);
    let dir = tempfile::tempdir().unwrap();

    let err = fx
        .app
        .register_created_triage_companion(&p, dir.path().to_path_buf())
        .unwrap_err()
        .to_string();

    assert!(err.contains("already exists"), "{err}");
    assert!(err.contains("Triage checkout retained at"), "{err}");
    assert_eq!(feature_count(&fx.app), 2);
}

#[test]
fn a_source_removed_after_planning_creates_no_worktree_and_runs_no_hook() {
    let mut fx = fixture(false, true);
    let hook = HookConfig::Script("echo ran > /dev/null; touch hook-ran.txt".into());
    let p = plan_with(
        &fx,
        "vanished",
        Some(&hook),
        None,
        false,
        FetchPolicy::Never,
    )
    .unwrap();
    fx.app.store.projects[0].features.clear();
    let before = git_dir_paths(&fx.repo);
    let branches_before = branches(&fx.repo);

    let err = fx
        .app
        .register_triage_companion(&p)
        .unwrap_err()
        .to_string();

    assert!(err.contains("Source feature removed"), "{err}");
    assert!(!err.contains("retained"), "nothing was created: {err}");
    assert_eq!(git_dir_paths(&fx.repo), before);
    assert_eq!(branches(&fx.repo), branches_before);
    assert!(!fx.repo.join("hook-ran.txt").exists());
}

#[test]
fn a_name_taken_after_planning_creates_no_worktree_and_runs_no_hook() {
    let mut fx = fixture(false, true);
    let hook = HookConfig::Script("touch hook-ran.txt".into());
    let p = plan_with(
        &fx,
        "raced-early",
        Some(&hook),
        None,
        false,
        FetchPolicy::Never,
    )
    .unwrap();
    let mut taken = fx.app.store.projects[0].features[0].clone();
    taken.id = "feat-raced-early".into();
    taken.name = "raced-early".into();
    taken.workdir = PathBuf::from("/tmp/elsewhere-raced-early");
    fx.app.store.projects[0].features.push(taken);
    let before = git_dir_paths(&fx.repo);
    let branches_before = branches(&fx.repo);

    let err = fx
        .app
        .register_triage_companion(&p)
        .unwrap_err()
        .to_string();

    assert!(err.contains("already exists"), "{err}");
    assert!(!err.contains("retained"), "nothing was created: {err}");
    assert_eq!(git_dir_paths(&fx.repo), before);
    assert_eq!(branches(&fx.repo), branches_before);
    assert!(!fx.repo.join("hook-ran.txt").exists());
}

#[test]
fn a_git_failure_creating_the_worktree_registers_nothing() {
    let mut fx = fixture(false, true);
    git(&fx.repo, &["branch", "taken-branch"]);
    let p = plan(&fx, "taken-branch");
    assert!(p.local_branch_exists, "the plan already shows the clash");
    let before = feature_count(&fx.app);

    let err = fx
        .app
        .register_triage_companion(&p)
        .unwrap_err()
        .to_string();

    assert!(err.contains("git worktree add failed"), "{err}");
    assert_eq!(feature_count(&fx.app), before);
}
