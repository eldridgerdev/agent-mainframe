use super::support::store_with_repo;
use crate::app::{App, AppMode, ProjectStatus, Selection};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crate::worktree::WorktreeManager;
use crossterm::event::KeyCode;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn fixture() -> TempDir {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(repo.path(), &["checkout", "-qb", "my-feat"]);
    std::fs::write(repo.path().join("file"), b"head").unwrap();
    std::fs::write(repo.path().join("amf.json"), r#"{"allowed_agents":["claude"],"lifecycle_hooks":{"on_worktree_created":{"script":"true","prompt":{"title":"Choose setup","options":["Continue"]}}}}"#).unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-qm", "initial"]);
    repo
}
fn app(repo: &TempDir, worktree: MockWorktreeOps) -> App {
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.store_path = repo.path().join("store.json");
    app.selection = Selection::Feature(0, 0);
    app
}
#[test]
fn fork_rejects_changed_source_and_missing_context_before_creating() {
    let repo = fixture();
    let mut app = app(&repo, MockWorktreeOps::new());
    app.start_fork_feature();
    assert!(matches!(app.mode, AppMode::ForkingFeature(_)));
    let error = app.create_forked_feature().unwrap_err();
    assert!(error.to_string().contains("No Claude transcript"));
    if let AppMode::ForkingFeature(state) = &mut app.mode {
        state.include_context = false;
    }
    std::fs::write(repo.path().join("file"), b"changed after preview").unwrap();
    assert!(app.create_forked_feature().is_err());
    assert!(!repo.path().join(".worktrees/my-feat-fork").exists());
}
#[test]
fn fork_keeps_stopped_feature_when_hook_prompt_is_cancelled() {
    let repo = fixture();
    std::fs::write(repo.path().join("file"), b"working content").unwrap();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_create_fork_base()
        .times(1)
        .returning(|repo, name, branch, commit| {
            WorktreeManager::create_fork_base(repo, name, branch, commit)
        });
    let mut app = app(&repo, worktree);
    app.start_fork_feature();
    crate::handlers::handle_fork_feature_key(&mut app, KeyCode::Enter).unwrap();
    crate::handlers::handle_fork_feature_key(&mut app, KeyCode::Tab).unwrap();
    crate::handlers::handle_fork_feature_key(&mut app, KeyCode::Enter).unwrap();
    assert!(matches!(app.mode, AppMode::HookPrompt(_)));
    let fork = &app.store.projects[0].features[1];
    assert_eq!(fork.status, ProjectStatus::Stopped);
    assert_eq!(
        std::fs::read(fork.workdir.join("file")).unwrap(),
        b"working content"
    );
    let id = fork.id.clone();
    crate::handlers::handle_hook_prompt_key(&mut app, KeyCode::Esc).unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .any(|f| f.id == id && f.status == ProjectStatus::Stopped)
    );
}

#[test]
fn fork_refuses_to_adopt_a_replaced_feature_at_hook_confirmation() {
    let repo = fixture();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_create_fork_base()
        .times(1)
        .returning(|repo, name, branch, commit| {
            WorktreeManager::create_fork_base(repo, name, branch, commit)
        });
    let mut app = app(&repo, worktree);
    app.start_fork_feature();
    crate::handlers::handle_fork_feature_key(&mut app, KeyCode::Enter).unwrap();
    crate::handlers::handle_fork_feature_key(&mut app, KeyCode::Tab).unwrap();
    crate::handlers::handle_fork_feature_key(&mut app, KeyCode::Enter).unwrap();
    assert!(matches!(app.mode, AppMode::HookPrompt(_)));
    let workdir = app.store.projects[0].features[1].workdir.clone();
    app.store.projects[0].features[1].id = "replacement-id".into();
    app.confirm_hook_prompt().unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("checkout retained")
    );
    assert!(workdir.exists());
    assert!(!app.store.projects[0].features[1].pending_worktree_script);
}

#[test]
fn fork_transfer_failure_retains_checkout_without_registering_or_starting() {
    let repo = fixture();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_create_fork_base()
        .times(1)
        .returning(|repo, name, branch, commit| {
            let path = WorktreeManager::create_fork_base(repo, name, branch, commit)?;
            std::fs::write(path.join("unexpected"), b"user content")?;
            Ok(path)
        });
    let mut app = app(&repo, worktree);
    app.start_fork_feature();
    if let AppMode::ForkingFeature(state) = &mut app.mode {
        state.include_context = false;
    }
    let error = app.create_forked_feature().unwrap_err().to_string();
    let dest = repo
        .path()
        .canonicalize()
        .unwrap()
        .join(".worktrees/my-feat-fork");
    assert!(error.contains(&dest.to_string_lossy().to_string()));
    assert!(error.contains("my-feat-fork") && error.contains("no agent started"));
    assert_eq!(
        std::fs::read(dest.join("unexpected")).unwrap(),
        b"user content"
    );
    assert_eq!(app.store.projects[0].features.len(), 1);
}

#[test]
fn fork_removes_checkout_and_branch_when_source_changes_mid_fork() {
    let repo = fixture();
    let source = repo.path().to_path_buf();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_create_fork_base()
        .times(1)
        .returning(move |repo, name, branch, commit| {
            let path = WorktreeManager::create_fork_base(repo, name, branch, commit)?;
            std::fs::write(source.join("edited-by-live-agent"), b"new")?;
            Ok(path)
        });
    worktree
        .expect_remove()
        .times(1)
        .returning(WorktreeManager::remove);
    let mut app = app(&repo, worktree);
    app.start_fork_feature();
    if let AppMode::ForkingFeature(state) = &mut app.mode {
        state.include_context = false;
    }
    let error = app.create_forked_feature().unwrap_err().to_string();
    assert!(error.contains("Checkout removed"), "{error}");
    let dest = repo
        .path()
        .canonicalize()
        .unwrap()
        .join(".worktrees/my-feat-fork");
    assert!(!dest.exists());
    let branches = std::process::Command::new("git")
        .args(["branch", "--list", "my-feat-fork"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(branches.stdout.is_empty());
}

#[test]
fn fork_registration_conflict_retains_checkout_without_starting() {
    let repo = fixture();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let writer = crate::db::AmfDb::open(&db_path).unwrap();
    writer
        .save_store(&store_with_repo(
            repo.path().to_path_buf(),
            ProjectStatus::Stopped,
        ))
        .unwrap();
    let (store, version) = writer.load_store_versioned().unwrap();
    let database_path = db_path.clone();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_create_fork_base()
        .times(1)
        .returning(move |repo, name, branch, commit| {
            let path = WorktreeManager::create_fork_base(repo, name, branch, commit)?;
            let writer = crate::db::AmfDb::open(&database_path)?;
            let mut changed = writer.load_store()?;
            changed.projects[0].collapsed = !changed.projects[0].collapsed;
            writer.save_store(&changed)?;
            Ok(path)
        });
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    app.store_version = Some(version);
    app.selection = Selection::Feature(0, 0);
    app.start_fork_feature();
    if let AppMode::ForkingFeature(state) = &mut app.mode {
        state.include_context = false;
    }
    let error = app.create_forked_feature().unwrap_err().to_string();
    let dest = repo
        .path()
        .canonicalize()
        .unwrap()
        .join(".worktrees/my-feat-fork");
    assert!(
        error.contains("registration conflicted")
            && error.contains(&dest.to_string_lossy().to_string())
    );
    assert!(dest.exists());
    assert_eq!(writer.load_store().unwrap().projects[0].features.len(), 1);
    assert!(!matches!(app.mode, AppMode::HookPrompt(_)));
}

#[test]
fn fork_refuses_session_retargeting_before_creation() {
    let repo = fixture();
    let mut app = app(&repo, MockWorktreeOps::new());
    app.store.projects[0].features[0]
        .add_session_named(crate::project::SessionKind::Claude, "Source".into());
    app.selection = Selection::Session(0, 0, 0);
    app.start_fork_feature();
    if let AppMode::ForkingFeature(state) = &mut app.mode {
        state.include_context = false;
    }
    app.store.projects[0].features[0].sessions[0].tmux_window = "replacement".into();
    assert!(
        app.create_forked_feature()
            .unwrap_err()
            .to_string()
            .contains("source session changed")
    );
    assert!(!repo.path().join(".worktrees/my-feat-fork").exists());
}
