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
    assert_eq!(
        feature.workdir,
        PathBuf::from(&git(
            &fx.repo,
            &["worktree", "list", "--porcelain"]
        )
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .find(|p| p.ends_with("my-project-pr-head-triage"))
        .unwrap())
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
    fx.app.pr_review_triage_setup_confirm().unwrap();

    assert!(setup_error(&fx.app).is_some());
    assert!(setup_open(&fx.app));
    assert_eq!(fx.calls.lock().unwrap().len(), 1);
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
        toast_texts(&fx.app).iter().any(|t| t
            == "Triage worktree created, but its on_worktree_created hook failed"),
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
