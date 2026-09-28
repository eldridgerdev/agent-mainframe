//! Cross-process store coordination from the `App` side: what a save does
//! when another process (the GUI, another TUI) committed first, and how the
//! TUI picks up such writes before it saves.

use super::support::*;
use crate::app::*;
use crate::db::AmfDb;
use crate::db::todos::{TodoPriority, TodoScope, TodoStatus, TodoWorkState};
use crate::project::{Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use tempfile::NamedTempFile;

/// A store whose feature already has one session, so a `Session` selection
/// has something to point at.
fn seeded_store() -> ProjectStore {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "Agent".into());
    store
}

/// An `App` that loaded `path` the way startup does: data and version from
/// one snapshot.
fn app_on(path: &std::path::Path, tmux: MockTmuxOps) -> App {
    let db = AmfDb::open(path).unwrap();
    let (store, version) = db.load_store_versioned().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_version = Some(version);
    app.db = Some(db);
    app
}

/// Another process's write: a project inserted *ahead* of the existing one,
/// so every index the `App` holds shifts by one.
fn prepend_project_elsewhere(writer: &AmfDb) {
    let mut store = writer.load_store().unwrap();
    let mut other = Project::new(
        "other".into(),
        std::path::PathBuf::from("/tmp/other"),
        false,
        Default::default(),
    );
    other.id = "proj-0".into();
    store.projects.insert(0, other);
    writer.save_store(&store).unwrap();
}

#[test]
fn a_conflicting_save_reloads_one_snapshot_and_keeps_the_selection_on_its_row() {
    let tmp = NamedTempFile::new().unwrap();
    let writer = AmfDb::open(tmp.path()).unwrap();
    writer.save_store(&seeded_store()).unwrap();
    let mut app = app_on(tmp.path(), MockTmuxOps::new());
    app.selection = Selection::Session(0, 0, 0);

    prepend_project_elsewhere(&writer);
    app.store.projects[0].features[0].nickname = Some("lost".into());

    assert!(!app.save_reporting_conflict().unwrap());

    // The other writer's state, at the version that describes it...
    assert_eq!(app.store.projects.len(), 2);
    assert_eq!(
        app.store_version,
        Some(writer.current_store_version().unwrap())
    );
    // ...with the selection still on the same session, now one project down.
    assert!(matches!(app.selection, Selection::Session(1, 0, 0)));
    // And the next save is not a spurious conflict.
    app.store.projects[1].features[0].nickname = Some("kept".into());
    assert!(app.save_reporting_conflict().unwrap());
}

#[test]
fn save_reapplying_re_adds_the_change_onto_the_other_writers_state() {
    let tmp = NamedTempFile::new().unwrap();
    let writer = AmfDb::open(tmp.path()).unwrap();
    writer.save_store(&seeded_store()).unwrap();
    let mut app = app_on(tmp.path(), MockTmuxOps::new());

    let session = app.store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "New".into())
        .clone();
    prepend_project_elsewhere(&writer);

    let outcome = app
        .save_reapplying(|store| {
            let Some((pi, fi)) = store.locate_feature_by_id(None, "feat-1") else {
                return false;
            };
            store.projects[pi].features[fi]
                .sessions
                .push(session.clone());
            true
        })
        .unwrap();

    assert_eq!(outcome, ReapplyOutcome::Saved);
    let on_disk = writer.load_store().unwrap();
    assert_eq!(
        on_disk.projects.len(),
        2,
        "the other writer's project survives"
    );
    assert!(
        on_disk.projects[1].features[0]
            .sessions
            .iter()
            .any(|s| s.id == session.id),
        "and so does this writer's session"
    );
}

#[test]
fn save_reapplying_reports_a_target_removed_elsewhere() {
    let tmp = NamedTempFile::new().unwrap();
    let writer = AmfDb::open(tmp.path()).unwrap();
    writer.save_store(&seeded_store()).unwrap();
    let mut app = app_on(tmp.path(), MockTmuxOps::new());

    app.store.projects[0].features[0].nickname = Some("mine".into());
    writer.save_store(&ProjectStore::empty()).unwrap();

    let outcome = app
        .save_reapplying(|store| store.locate_feature_by_id(None, "feat-1").is_some())
        .unwrap();

    assert_eq!(outcome, ReapplyOutcome::TargetGone);
    assert!(app.store.projects.is_empty());
    assert!(writer.load_store().unwrap().projects.is_empty());
}

#[test]
fn a_new_agent_session_survives_a_concurrent_write_instead_of_orphaning_its_window() {
    let tmp = NamedTempFile::new().unwrap();
    let writer = AmfDb::open(tmp.path()).unwrap();
    writer.save_store(&seeded_store()).unwrap();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    // The other process commits while the harness is starting: after the
    // window exists, before its record is saved.
    let path = tmp.path().to_path_buf();
    tmux.expect_launch_claude().returning(move |_, _, _, _, _| {
        prepend_project_elsewhere(&AmfDb::open(&path).unwrap());
        Ok(())
    });
    // Nothing may be torn down: the record is re-applied, not dropped.
    tmux.expect_kill_window().never();
    let mut app = app_on(tmp.path(), tmux);

    let (si, session_id, _) = app
        .create_agent_session_labeled_identified(0, 0, "Agent 2", None, StartIntent::Approved)
        .unwrap();

    assert_eq!(app.session_indices_by_id(&session_id), Some((1, 0, si)));
    let on_disk = writer.load_store().unwrap();
    assert_eq!(on_disk.projects.len(), 2);
    assert!(
        on_disk.projects[1].features[0]
            .sessions
            .iter()
            .any(|s| s.id == session_id)
    );
}

#[test]
fn a_new_agent_session_whose_feature_was_deleted_elsewhere_is_torn_down() {
    let tmp = NamedTempFile::new().unwrap();
    let writer = AmfDb::open(tmp.path()).unwrap();
    writer.save_store(&seeded_store()).unwrap();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    let path = tmp.path().to_path_buf();
    tmux.expect_launch_claude().returning(move |_, _, _, _, _| {
        AmfDb::open(&path)
            .unwrap()
            .save_store(&ProjectStore::empty())
            .unwrap();
        Ok(())
    });
    tmux.expect_kill_window().times(1).returning(|_, _| Ok(()));
    let mut app = app_on(tmp.path(), tmux);

    let error = app
        .create_agent_session_labeled_identified(0, 0, "Agent 2", None, StartIntent::Approved)
        .unwrap_err();

    assert!(error.to_string().contains("removed elsewhere"), "{error}");
    assert!(app.store.projects.is_empty());
}

#[test]
fn the_tui_adopts_external_writes_only_where_no_indices_are_held() {
    let tmp = NamedTempFile::new().unwrap();
    let writer = AmfDb::open(tmp.path()).unwrap();
    writer.save_store(&seeded_store()).unwrap();
    let mut app = app_on(tmp.path(), MockTmuxOps::new());
    app.selection = Selection::Feature(0, 0);

    assert!(!app.refresh_store_if_changed_elsewhere().unwrap());

    prepend_project_elsewhere(&writer);
    app.mode = AppMode::DeletingProject("my-project".into());
    assert!(
        !app.refresh_store_if_changed_elsewhere().unwrap(),
        "a modal may hold indices; wait for it to close"
    );
    assert_eq!(app.store.projects.len(), 1);

    app.mode = AppMode::Normal;
    assert!(app.refresh_store_if_changed_elsewhere().unwrap());
    assert_eq!(app.store.projects.len(), 2);
    assert!(matches!(app.selection, Selection::Feature(1, 0)));
    // A save straight after the refresh does not conflict.
    assert!(app.save_reporting_conflict().unwrap());
}

#[test]
fn a_launch_that_linked_its_session_before_failing_still_rolls_back_on_disk() {
    let tmp = NamedTempFile::new().unwrap();
    let db = AmfDb::open(tmp.path()).unwrap();
    db.save_store(&seeded_store()).unwrap();
    let list = db
        .create_todo_list(
            &TodoScope::Project {
                project_id: "proj-1".into(),
            },
            Some("feat-1"),
        )
        .unwrap();
    let todo = db
        .add_todo(&list.id, "work", None, TodoPriority::Med)
        .unwrap();
    let mut app = app_on(tmp.path(), MockTmuxOps::new());

    assert!(app.todos_reserve_launch(&todo).unwrap());
    app.todos_mark_in_progress(&todo.id, Some("session-created"))
        .unwrap();
    // e.g. the composer could not be seeded.
    app.todos_rollback_launch(&todo.id, Some("session-created"))
        .unwrap();

    let reloaded = app.db.as_ref().unwrap().find_todo_by_id(&todo.id).unwrap();
    assert_eq!(reloaded.unwrap().work, TodoWorkState::default());

    // A link to some other session is someone else's state and is kept.
    assert!(app.todos_reserve_launch(&todo).unwrap());
    app.todos_mark_in_progress(&todo.id, Some("someone-else"))
        .unwrap();
    app.todos_rollback_launch(&todo.id, Some("session-created"))
        .unwrap();
    let kept = app
        .db
        .as_ref()
        .unwrap()
        .find_todo_by_id(&todo.id)
        .unwrap()
        .unwrap();
    assert_eq!(kept.work.status, TodoStatus::InProgress);
    assert_eq!(kept.work.agent_session_id.as_deref(), Some("someone-else"));
}
