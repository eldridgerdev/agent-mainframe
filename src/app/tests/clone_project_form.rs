//! The New Project form's optional Git URL: derivation, the pre-flight and
//! progress screens, and that an empty URL leaves the old flow untouched.

use crate::app::clone_project::{
    CloneCheck, CloneCheckStatus, CloneFix, ClonePhase, CloneProgress,
};
use crate::app::*;
use crate::git_clone::env::GitEnv;
use crate::project::ProjectStore;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 40).unwrap();
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn new_app() -> App {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore::empty(),
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

fn form(app: &App) -> &CreateProjectState {
    match &app.mode {
        AppMode::CreatingProject(s) => s,
        _ => panic!("expected the New Project form"),
    }
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

fn remote(root: &Path) -> String {
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

/// Poll like the main loop does until `done` holds.
fn pump(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_clone_preflight();
        app.poll_clone_progress();
        app.sync_clone_project_mode();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn filled_form(url: &str, name: &str, path: &Path) -> CreateProjectState {
    CreateProjectState {
        step: CreateProjectStep::Agent,
        url: url.into(),
        name: name.into(),
        path: path.to_string_lossy().into_owned(),
        ..CreateProjectState::blank()
    }
}

#[test]
fn empty_url_flow_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = new_app();
    app.start_create_project();
    assert_eq!(form(&app).step, CreateProjectStep::Name);
    assert!(form(&app).url.is_empty());

    type_text(&mut app, "plain");
    press(&mut app, KeyCode::Tab);
    assert_eq!(form(&app).step, CreateProjectStep::Path);
    type_text(&mut app, tmp.path().to_str().unwrap());
    press(&mut app, KeyCode::Tab);
    assert_eq!(form(&app).step, CreateProjectStep::Agent);
    // Nothing was derived or overwritten.
    assert_eq!(form(&app).name, "plain");
    assert_eq!(form(&app).path, tmp.path().to_str().unwrap());

    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, AppMode::Normal), "registered directly");
    assert_eq!(app.store.projects.len(), 1);
    assert_eq!(app.store.projects[0].name, "plain");
    assert_eq!(app.store.projects[0].repo, tmp.path());
    assert_eq!(app.clone_project_snapshot().phase, ClonePhase::Idle);
}

#[test]
fn clearing_the_url_restores_the_blank_form() {
    let mut app = new_app();
    app.config.projects.projects_root = Some("/work/repos".into());
    app.start_create_project();
    press(&mut app, KeyCode::BackTab);
    assert_eq!(form(&app).step, CreateProjectStep::Url);
    type_text(&mut app, "https://github.com/o/widget.git");
    assert_eq!(form(&app).name, "widget");
    assert_eq!(form(&app).path, "/work/repos/widget");
    assert!(form(&app).url_error.is_none());

    // Ctrl+U clears the focused field, and with it the derived values.
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        40,
    )
    .unwrap();
    assert!(form(&app).url.is_empty());
    assert!(form(&app).name.is_empty() && form(&app).path.is_empty());
}

#[test]
fn edits_to_derived_values_persist_across_url_changes() {
    let mut app = new_app();
    app.config.projects.projects_root = Some("/work/repos".into());
    app.start_create_project();
    press(&mut app, KeyCode::BackTab);
    type_text(&mut app, "https://github.com/o/widget");
    assert_eq!(form(&app).name, "widget");

    // The user edits the name, then the directory.
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "-mine");
    assert_eq!(form(&app).name, "widget-mine");
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "/sub");
    assert_eq!(form(&app).path, "/work/repos/widget/sub");

    // Changing the URL no longer overwrites either.
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert_eq!(form(&app).step, CreateProjectStep::Url);
    type_text(&mut app, ".git");
    assert_eq!(form(&app).name, "widget-mine");
    assert_eq!(form(&app).path, "/work/repos/widget/sub");
    // ...and clearing the URL leaves the user's own values alone.
    for _ in 0..40 {
        press(&mut app, KeyCode::Backspace);
    }
    assert_eq!(form(&app).name, "widget-mine");
    assert_eq!(form(&app).path, "/work/repos/widget/sub");
}

#[test]
fn bad_url_is_explained_and_enter_stays_on_the_form() {
    let mut app = new_app();
    app.start_create_project();
    press(&mut app, KeyCode::BackTab);
    type_text(&mut app, "ext::sh -c evil");
    assert!(form(&app).url_error.is_some());
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "x");
    app.mode = AppMode::CreatingProject(CreateProjectState {
        name: "x".into(),
        path: "/tmp/x".into(),
        ..filled_form("ext::sh", "x", Path::new("/tmp/x"))
    });
    press(&mut app, KeyCode::Enter);
    let f = form(&app);
    assert_eq!(f.step, CreateProjectStep::Url);
    assert!(app.message.as_deref().unwrap().starts_with("Error:"));
    assert_eq!(f.name, "x", "values preserved");
}

#[test]
fn existing_destination_returns_to_the_path_field() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = new_app();
    app.mode = AppMode::CreatingProject(filled_form(
        "https://example.com/o/repo.git",
        "repo",
        tmp.path(),
    ));
    press(&mut app, KeyCode::Enter);
    assert_eq!(form(&app).step, CreateProjectStep::Path);
    assert!(app.message.as_deref().unwrap().starts_with("Error:"));
}

fn arm_clone_view(app: &mut App, f: CreateProjectState, phase: ClonePhase, url: &str) {
    app.set_clone_phase_for_test(phase, Some(url));
    app.mode = AppMode::CloneProject(Box::new(CloneProjectViewState {
        form: f,
        notice: None,
        cancelling: false,
    }));
}

fn check(status: CloneCheckStatus, fix: Option<CloneFix>) -> CloneCheck {
    CloneCheck {
        id: "gh_auth".into(),
        status,
        message: "msg".into(),
        fix,
    }
}

#[test]
fn full_flow_clones_registers_and_selects_the_project() {
    let tmp = tempfile::tempdir().unwrap();
    let url = remote(tmp.path());
    let dest = tmp.path().join("clones").join("remote");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let mut app = new_app();
    app.set_clone_project_env_for_test(GitEnv::new().with_file_transport(true), true);
    app.mode = AppMode::CreatingProject(filled_form(&url, "remote", &dest));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, AppMode::CloneProject(_)));
    pump(&mut app, "pre-flight", |a| {
        matches!(
            a.clone_project_snapshot().phase,
            ClonePhase::PreflightDone { .. }
        )
    });
    assert!(app.clone_project_snapshot().can_start);
    press(&mut app, KeyCode::Enter);
    pump(&mut app, "clone", |a| matches!(a.mode, AppMode::Normal));
    assert_eq!(app.store.projects.len(), 1);
    assert_eq!(app.store.projects[0].name, "remote");
    assert!(matches!(app.selection, Selection::Project(0)));
    assert!(dest.join("README").exists());
}

#[test]
fn fail_disables_start_and_warn_allows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let url = remote(tmp.path());
    let dest = tmp.path().join("remote-clone");
    let mut app = new_app();
    app.set_clone_project_env_for_test(GitEnv::new().with_file_transport(true), true);

    let blocked = ClonePhase::PreflightDone {
        checks: vec![check(CloneCheckStatus::Fail, None)],
        default_branch: None,
        blocked: true,
    };
    arm_clone_view(&mut app, filled_form(&url, "r", &dest), blocked, &url);
    // A stale `can_start` is not enough: the phase says blocked.
    assert!(!app.clone_project_snapshot().can_start);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.clone_project_snapshot().phase,
        ClonePhase::PreflightDone { blocked: true, .. }
    ));
    match &app.mode {
        AppMode::CloneProject(v) => assert!(v.notice.as_deref().unwrap().contains("disabled")),
        _ => panic!("still on the pre-flight screen"),
    }
    assert!(!dest.exists());

    let warned = ClonePhase::PreflightDone {
        checks: vec![check(CloneCheckStatus::Warn, None)],
        default_branch: None,
        blocked: false,
    };
    arm_clone_view(&mut app, filled_form(&url, "r", &dest), warned, &url);
    assert!(app.clone_project_snapshot().can_start);
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(
            app.clone_project_snapshot().phase,
            ClonePhase::Cloning { .. } | ClonePhase::Registering | ClonePhase::Succeeded { .. }
        ),
        "a warning does not block Start"
    );
    pump(&mut app, "clone", |a| matches!(a.mode, AppMode::Normal));
}

#[test]
fn fix_commands_are_numbered_and_copy_only() {
    let mut app = new_app();
    let fix = |id: &str, cmd: &str| CloneFix {
        id: id.into(),
        label: id.into(),
        command_line: cmd.into(),
    };
    let phase = ClonePhase::PreflightDone {
        checks: vec![
            check(CloneCheckStatus::Fail, Some(fix("a", "gh auth login"))),
            check(CloneCheckStatus::Pass, None),
            check(CloneCheckStatus::Warn, Some(fix("b", "ssh-add"))),
        ],
        default_branch: None,
        blocked: true,
    };
    arm_clone_view(
        &mut app,
        filled_form("https://e.com/o/r.git", "r", Path::new("/tmp/r")),
        phase,
        "https://e.com/o/r.git",
    );
    assert_eq!(
        app.clone_view_fix_command(1).unwrap().command_line,
        "gh auth login"
    );
    assert_eq!(
        app.clone_view_fix_command(2).unwrap().command_line,
        "ssh-add"
    );
    assert!(app.clone_view_fix_command(3).is_none());
    assert!(app.clone_view_fix_command(0).is_none());
}

#[test]
fn esc_on_preflight_returns_to_the_form_with_values() {
    let mut app = new_app();
    let f = filled_form("https://e.com/o/r.git", "r", Path::new("/tmp/r"));
    let phase = ClonePhase::PreflightDone {
        checks: vec![],
        default_branch: None,
        blocked: false,
    };
    arm_clone_view(&mut app, f, phase, "https://e.com/o/r.git");
    press(&mut app, KeyCode::Esc);
    let f = form(&app);
    assert_eq!(
        (f.url.as_str(), f.name.as_str(), f.path.as_str()),
        ("https://e.com/o/r.git", "r", "/tmp/r")
    );
    assert_eq!(app.clone_project_snapshot().phase, ClonePhase::Idle);
}

fn slow_git(tmp: &Path) -> PathBuf {
    let bin = tmp.join("fakegit");
    std::fs::write(
        &bin,
        "#!/bin/sh\nprintf 'Receiving objects:  10%% (1/10)\\r' >&2\nsleep 300\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
fn esc_during_progress_cancels_and_returns_to_the_form() {
    let tmp = tempfile::tempdir().unwrap();
    let url = "https://example.com/o/repo.git";
    let dest = tmp.path().join("repo");
    let mut app = new_app();
    app.set_clone_project_env_for_test(GitEnv::new().with_git_binary(slow_git(tmp.path())), false);
    let ok = ClonePhase::PreflightDone {
        checks: vec![],
        default_branch: None,
        blocked: false,
    };
    arm_clone_view(&mut app, filled_form(url, "repo", &dest), ok, url);
    press(&mut app, KeyCode::Enter);
    pump(&mut app, "progress", |a| {
        matches!(
            a.clone_project_snapshot().phase,
            ClonePhase::Cloning {
                progress: Some(CloneProgress { .. })
            }
        )
    });
    assert!(dest.is_dir());
    press(&mut app, KeyCode::Esc);
    pump(&mut app, "form", |a| {
        matches!(a.mode, AppMode::CreatingProject(_))
    });
    let f = form(&app);
    assert_eq!((f.url.as_str(), f.name.as_str()), (url, "repo"));
    assert_eq!(f.path, dest.to_string_lossy());
    assert!(app.message.as_deref().unwrap().contains("cancelled"));
    assert!(!dest.exists(), "owned directory removed");
    assert!(app.store.projects.is_empty());
}

#[test]
fn cancel_is_unavailable_while_registering() {
    let mut app = new_app();
    let f = filled_form("https://e.com/o/r.git", "r", Path::new("/tmp/r"));
    arm_clone_view(
        &mut app,
        f,
        ClonePhase::Registering,
        "https://e.com/o/r.git",
    );
    press(&mut app, KeyCode::Esc);
    match &app.mode {
        AppMode::CloneProject(v) => {
            assert!(v.notice.as_deref().unwrap().contains("can't be cancelled"));
            assert!(!v.cancelling);
        }
        _ => panic!("must stay on the progress screen"),
    }
}

#[test]
fn clone_failure_returns_to_the_form_with_the_message() {
    let tmp = tempfile::tempdir().unwrap();
    let url = format!("file://{}/missing.git", tmp.path().display());
    let dest = tmp.path().join("missing");
    let mut app = new_app();
    app.set_clone_project_env_for_test(GitEnv::new().with_file_transport(true), true);
    let ok = ClonePhase::PreflightDone {
        checks: vec![],
        default_branch: None,
        blocked: false,
    };
    arm_clone_view(&mut app, filled_form(&url, "missing", &dest), ok, &url);
    press(&mut app, KeyCode::Enter);
    pump(&mut app, "form", |a| {
        matches!(a.mode, AppMode::CreatingProject(_))
    });
    assert!(app.message.as_deref().unwrap().starts_with("Error:"));
    assert_eq!(form(&app).name, "missing");
    assert!(!dest.exists());
}

// ---------------------------------------------------- missing projects root

const ROOT_URL: &str = "https://github.com/o/widget.git";

fn prompt(app: &App) -> &CloneRootPromptState {
    match &app.mode {
        AppMode::CloneRootPrompt(p) => p,
        _ => panic!("expected the missing projects folder prompt"),
    }
}

/// An app whose projects root is `root` and whose form is submitted.
fn submit_with_root(root: &Path) -> (App, PathBuf) {
    let mut app = new_app();
    app.config.projects.projects_root = Some(root.to_string_lossy().into_owned());
    let dest = root.join("widget");
    let mut f = filled_form(ROOT_URL, "widget", &dest);
    f.name_derived = true;
    f.path_derived = true;
    app.mode = AppMode::CreatingProject(f);
    press(&mut app, KeyCode::Enter);
    (app, dest)
}

#[test]
fn missing_root_opens_prompt_preserving_the_form() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let (app, dest) = submit_with_root(&root);
    let p = prompt(&app);
    assert_eq!(p.root, root);
    assert_eq!(p.form.url, ROOT_URL);
    assert_eq!(p.form.name, "widget");
    assert_eq!(p.form.path, dest.to_string_lossy());
    assert!(p.editing.is_none() && p.error.is_none());
    assert!(!root.exists());
}

#[test]
fn create_makes_only_the_root_and_enters_the_preflight() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let (mut app, dest) = submit_with_root(&root);
    app.set_clone_project_env_for_test(GitEnv::new().with_file_transport(true), true);
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, AppMode::CloneProject(_)));
    assert!(root.is_dir());
    assert!(!dest.exists(), "only the projects folder is created");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    app.shutdown_clone_job();
}

#[test]
fn enter_also_creates_and_a_failure_stays_in_the_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let (mut app, _dest) = submit_with_root(&root);
    // Something appears at the path meanwhile: creation is refused, prompt stays.
    std::fs::write(&root, "file").unwrap();
    press(&mut app, KeyCode::Enter);
    let p = prompt(&app);
    assert!(p.error.as_deref().unwrap().contains("not a directory"));
    assert_eq!(std::fs::read_to_string(&root).unwrap(), "file");
}

#[test]
fn choose_different_folder_persists_rederives_and_returns_to_the_form() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let (mut app, _dest) = submit_with_root(&root);
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(
        prompt(&app).editing.as_deref(),
        Some(root.to_str().unwrap()),
        "prefilled with the current root"
    );
    // Relative path: error, still in the prompt, nothing changed.
    app.mode = match std::mem::replace(&mut app.mode, AppMode::Normal) {
        AppMode::CloneRootPrompt(mut p) => {
            p.editing = Some("relative".into());
            AppMode::CloneRootPrompt(p)
        }
        m => m,
    };
    press(&mut app, KeyCode::Enter);
    assert!(prompt(&app).error.is_some());
    assert_eq!(
        app.config.projects.projects_root.as_deref(),
        Some(root.to_str().unwrap())
    );

    app.mode = match std::mem::replace(&mut app.mode, AppMode::Normal) {
        AppMode::CloneRootPrompt(mut p) => {
            p.editing = Some(String::new());
            AppMode::CloneRootPrompt(p)
        }
        m => m,
    };
    type_text(&mut app, other.to_str().unwrap());
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.config.projects.projects_root.as_deref(),
        Some(other.to_str().unwrap())
    );
    assert_eq!(form(&app).path, other.join("widget").to_string_lossy());
    assert_eq!(form(&app).url, ROOT_URL);
    assert!(!root.exists());
}

#[test]
fn choose_keeps_a_hand_edited_directory_and_new_missing_root_reprompts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let other = tmp.path().join("other");
    let manual = other.join("mine");
    let (mut app, _dest) = submit_with_root(&root);
    // Return to the form and hand-edit the directory under the new root.
    press(&mut app, KeyCode::Char('d'));
    app.mode = match std::mem::replace(&mut app.mode, AppMode::Normal) {
        AppMode::CloneRootPrompt(mut p) => {
            p.form.path = manual.to_string_lossy().into_owned();
            p.form.path_derived = false;
            p.editing = Some(other.to_string_lossy().into_owned());
            AppMode::CloneRootPrompt(p)
        }
        m => m,
    };
    press(&mut app, KeyCode::Enter);
    assert_eq!(form(&app).path, manual.to_string_lossy(), "edit kept");
    // Clear the manual flag path and re-derive: submit with the derived dir
    // under the (still missing) new root prompts again.
    if let AppMode::CreatingProject(f) = &mut app.mode {
        f.path = other.join("widget").to_string_lossy().into_owned();
        f.path_derived = true;
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(prompt(&app).root, other);
    assert!(!other.exists());
}

#[test]
fn esc_restores_the_form_verbatim_and_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let (mut app, dest) = submit_with_root(&root);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Esc);
    assert!(
        prompt(&app).editing.is_none(),
        "Esc leaves the text field first"
    );
    press(&mut app, KeyCode::Esc);
    let f = form(&app);
    assert_eq!(f.url, ROOT_URL);
    assert_eq!(f.name, "widget");
    assert_eq!(f.path, dest.to_string_lossy());
    assert!(f.name_derived && f.path_derived);
    assert_eq!(f.step, CreateProjectStep::Agent);
    assert!(!root.exists());
}

#[test]
fn missing_non_root_parent_shows_the_old_error_without_a_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let mut app = new_app();
    app.config.projects.projects_root = Some(root.to_string_lossy().into_owned());
    let dest = tmp.path().join("elsewhere").join("widget");
    app.mode = AppMode::CreatingProject(filled_form(ROOT_URL, "widget", &dest));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, AppMode::CreatingProject(_)));
    assert!(app.message.as_deref().unwrap().starts_with("Error:"));
    assert!(!tmp.path().join("elsewhere").exists());
}

fn ctrl(app: &mut App, c: char) {
    crate::handlers::handle_key(
        app,
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
        40,
    )
    .unwrap();
}

#[test]
fn ctrl_o_toggles_container_mode_without_typing_into_a_field() {
    let mut app = new_app();
    app.start_create_project();
    assert!(!form(&app).containerized);
    ctrl(&mut app, 'o');
    assert!(form(&app).containerized);
    assert!(form(&app).name.is_empty(), "Ctrl+O must not type an 'o'");
    ctrl(&mut app, 'o');
    assert!(!form(&app).containerized);
}

#[test]
fn container_mode_refuses_ssh_urls_and_says_why() {
    let mut app = new_app();
    app.start_create_project();
    press(&mut app, KeyCode::BackTab); // to the URL field
    type_text(&mut app, "git@github.com:o/r.git");
    assert_eq!(form(&app).url_error, None, "fine without a container");
    assert_eq!(form(&app).name, "r");

    ctrl(&mut app, 'o');
    let err = form(&app)
        .url_error
        .clone()
        .expect("ssh refused in container mode");
    assert!(err.contains("https://"), "{err}");

    // Back to https: the error clears and a name is derived again.
    for _ in 0..40 {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "https://github.com/o/r.git");
    assert_eq!(form(&app).url_error, None);
    assert_eq!(form(&app).name, "r");
}

#[test]
fn toggling_container_mode_discards_a_finished_preflight() {
    let mut app = new_app();
    app.set_clone_phase_for_test(
        ClonePhase::PreflightDone {
            checks: vec![],
            default_branch: Some("main".into()),
            blocked: false,
        },
        Some("https://github.com/o/r.git"),
    );
    assert!(app.clone_project_snapshot().can_start);
    app.set_clone_containerized(true);
    assert!(
        !app.clone_project_snapshot().can_start,
        "the host git's pre-flight says nothing about the sandbox"
    );
}
